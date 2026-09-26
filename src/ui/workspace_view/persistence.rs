//! Ordered persistence off the GPUI thread. The final message carries the
//! latest state, so a delayed write cannot replace it during window teardown.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
#[cfg(test)]
use std::time::Duration;

use async_channel::Receiver;

use crate::domain::library::Library;
use crate::domain::workspace::WorkspaceSnapshot;
use crate::infrastructure::library::LibraryRepository;
use crate::infrastructure::persistence::WorkspaceRepository;
use crate::infrastructure::settings::{AppSettings, SettingsRepository};

#[cfg(test)]
const TEST_IDLE_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) enum SaveResult {
    Workspace {
        generation: u64,
        error: Option<String>,
    },
    Settings {
        generation: u64,
        error: Option<String>,
    },
    Library {
        generation: u64,
        error: Option<String>,
    },
}

#[derive(Debug)]
pub(super) enum FinishError {
    Unavailable,
    Save(String),
}

enum Command {
    Wake,
    Finish {
        workspace: Box<Option<(u64, WorkspaceSnapshot)>>,
        settings: Option<(u64, AppSettings)>,
        library: Option<(u64, Library)>,
        completed: mpsc::Sender<Vec<String>>,
    },
    #[cfg(test)]
    Barrier(mpsc::Sender<()>),
    Stop,
}

#[derive(Default)]
struct PendingWrites {
    workspace: Option<(u64, WorkspaceSnapshot)>,
    settings: Option<(u64, AppSettings)>,
    library: Option<(u64, Library)>,
}

pub(super) struct PersistenceQueue {
    commands: mpsc::Sender<Command>,
    pending: Arc<Mutex<PendingWrites>>,
    wake_queued: Arc<AtomicBool>,
    _worker: thread::JoinHandle<()>,
}

impl PersistenceQueue {
    pub fn start(
        workspace_repository: WorkspaceRepository,
        settings_repository: SettingsRepository,
        library_repository: Option<LibraryRepository>,
    ) -> io::Result<(Self, Receiver<SaveResult>)> {
        let (commands, receiver) = mpsc::channel();
        let (results, result_receiver) = async_channel::unbounded();
        let pending = Arc::new(Mutex::new(PendingWrites::default()));
        let worker_pending = pending.clone();
        let wake_queued = Arc::new(AtomicBool::new(false));
        let worker_wake_queued = wake_queued.clone();
        let worker = thread::Builder::new()
            .name("vibra-persistence".into())
            .spawn(move || {
                run(
                    receiver,
                    worker_pending,
                    worker_wake_queued,
                    results,
                    workspace_repository,
                    settings_repository,
                    library_repository,
                )
            })?;
        Ok((
            Self {
                commands,
                pending,
                wake_queued,
                _worker: worker,
            },
            result_receiver,
        ))
    }

    pub fn save_workspace(
        &self,
        generation: u64,
        snapshot: WorkspaceSnapshot,
    ) -> Result<(), String> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .workspace = Some((generation, snapshot));
        self.wake()
            .map_err(|_| "el guardado de proyectos ya no está disponible".into())
    }

    pub fn save_settings(&self, generation: u64, settings: AppSettings) -> Result<(), String> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .settings = Some((generation, settings));
        self.wake()
            .map_err(|_| "el guardado de settings ya no está disponible".into())
    }

    fn wake(&self) -> Result<(), ()> {
        if self.wake_queued.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        if self.commands.send(Command::Wake).is_err() {
            self.wake_queued.store(false, Ordering::Release);
            return Err(());
        }
        Ok(())
    }

    pub fn save_library(&self, generation: u64, library: Library) -> Result<(), String> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .library = Some((generation, library));
        self.wake()
            .map_err(|_| "el guardado de notas y automatizaciones ya no está disponible".into())
    }

    /// Wait for the last state to reach disk before the process can exit. A
    /// deadline here would silently discard changes if storage was slow.
    pub fn finish(
        &self,
        workspace: Option<(u64, WorkspaceSnapshot)>,
        settings: Option<(u64, AppSettings)>,
        library: Option<(u64, Library)>,
    ) -> Result<(), FinishError> {
        let (completed, reply) = mpsc::channel();
        self.commands
            .send(Command::Finish {
                workspace: Box::new(workspace),
                settings,
                library,
                completed,
            })
            .map_err(|_| FinishError::Unavailable)?;
        let errors = reply.recv().map_err(|_| FinishError::Unavailable)?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(FinishError::Save(errors.join("; ")))
        }
    }

    #[cfg(test)]
    pub fn wait_for_idle(&self) {
        let (completed, reply) = mpsc::channel();
        self.commands.send(Command::Barrier(completed)).unwrap();
        reply.recv_timeout(TEST_IDLE_TIMEOUT).unwrap();
    }
}

impl Drop for PersistenceQueue {
    fn drop(&mut self) {
        // The thread handle detaches on drop. This signal lets it finish any
        // pending writes if the view was released without calling `finish`.
        let _ = self.commands.send(Command::Stop);
    }
}

/// Emergency path when the queue could not start or has stopped. The final
/// save completes before the process exits, even if storage is slow.
pub(super) fn save_final_blocking(
    workspace_repository: WorkspaceRepository,
    settings_repository: SettingsRepository,
    library_repository: Option<LibraryRepository>,
    workspace: Option<WorkspaceSnapshot>,
    settings: Option<AppSettings>,
    library: Option<Library>,
) -> Result<(), FinishError> {
    let mut errors = Vec::new();
    if let Some(workspace) = workspace
        && let Err(error) = workspace_repository.save(&workspace)
    {
        errors.push(format!("No se pudieron guardar proyectos: {error}"));
    }
    if let Some(settings) = settings
        && let Err(error) = settings_repository.save(&settings)
    {
        errors.push(format!("No se pudieron guardar settings: {error}"));
    }
    if let Some(library) = library
        && let Some(repository) = library_repository
        && let Err(error) = repository.save(&library)
    {
        errors.push(format!(
            "No se pudieron guardar las notas y automatizaciones: {error}"
        ));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(FinishError::Save(errors.join("; ")))
    }
}

fn run(
    commands: mpsc::Receiver<Command>,
    pending: Arc<Mutex<PendingWrites>>,
    wake_queued: Arc<AtomicBool>,
    results: async_channel::Sender<SaveResult>,
    workspace_repository: WorkspaceRepository,
    settings_repository: SettingsRepository,
    library_repository: Option<LibraryRepository>,
) {
    while let Ok(command) = commands.recv() {
        if matches!(command, Command::Wake) {
            // Clear before taking pending state: a concurrent update must be
            // able to queue another wake while this batch is being written.
            wake_queued.store(false, Ordering::Release);
        }
        let (mut workspace, mut settings, mut library) = {
            let mut pending = pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                pending.workspace.take(),
                pending.settings.take(),
                pending.library.take(),
            )
        };
        let (completed, stop) = match command {
            Command::Wake => (None, false),
            Command::Finish {
                workspace: final_workspace,
                settings: final_settings,
                library: final_library,
                completed,
            } => {
                workspace = (*final_workspace).or(workspace);
                settings = final_settings.or(settings);
                library = final_library.or(library);
                (Some(completed), true)
            }
            #[cfg(test)]
            Command::Barrier(barrier) => {
                persist_batch(
                    workspace,
                    settings,
                    library,
                    &workspace_repository,
                    &settings_repository,
                    library_repository.as_ref(),
                    &results,
                );
                let _ = barrier.send(());
                continue;
            }
            Command::Stop => (None, true),
        };
        let errors = persist_batch(
            workspace,
            settings,
            library,
            &workspace_repository,
            &settings_repository,
            library_repository.as_ref(),
            &results,
        );
        if let Some(completed) = completed {
            let _ = completed.send(errors);
        }
        if stop {
            break;
        }
    }
}

fn persist_batch(
    workspace: Option<(u64, WorkspaceSnapshot)>,
    settings: Option<(u64, AppSettings)>,
    library: Option<(u64, Library)>,
    workspace_repository: &WorkspaceRepository,
    settings_repository: &SettingsRepository,
    library_repository: Option<&LibraryRepository>,
    results: &async_channel::Sender<SaveResult>,
) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some((generation, snapshot)) = workspace {
        let error = workspace_repository
            .save(&snapshot)
            .err()
            .map(|error| format!("No se pudieron guardar proyectos: {error}"));
        if let Some(error) = &error {
            errors.push(error.clone());
        }
        let _ = results.try_send(SaveResult::Workspace { generation, error });
    }
    if let Some((generation, settings)) = settings {
        let error = settings_repository
            .save(&settings)
            .err()
            .map(|error| format!("No se pudieron guardar settings: {error}"));
        if let Some(error) = &error {
            errors.push(error.clone());
        }
        let _ = results.try_send(SaveResult::Settings { generation, error });
    }
    if let Some((generation, library)) = library {
        let error = match library_repository {
            Some(repository) => repository.save(&library).err().map(|error| {
                format!("No se pudieron guardar las notas y automatizaciones: {error}")
            }),
            None => Some("El guardado de notas y automatizaciones no está disponible".into()),
        };
        if let Some(error) = &error {
            errors.push(error.clone());
        }
        let _ = results.try_send(SaveResult::Library { generation, error });
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::paths::with_exclusive_file_lock;
    use std::path::Path;
    use uuid::Uuid;

    #[test]
    fn final_state_wins_over_queued_older_snapshots() {
        let root = std::env::temp_dir().join(format!("vibra-persistence-{}", Uuid::new_v4()));
        let workspace_repository = WorkspaceRepository::at(root.join("workspace.json"));
        let settings_repository = SettingsRepository::at(root.join("settings.json"));
        let library_repository = LibraryRepository::in_directory(&root);
        let (queue, _) = PersistenceQueue::start(
            workspace_repository.clone(),
            settings_repository.clone(),
            Some(library_repository.clone()),
        )
        .unwrap();
        let mut older = WorkspaceSnapshot::default();
        older.create_workspace(Path::new("/tmp/old"));
        let mut latest = older.clone();
        latest.create_workspace(Path::new("/tmp/latest"));
        let older_settings = AppSettings {
            terminal_font_size: 12.0,
            ..AppSettings::default()
        };
        let latest_settings = AppSettings {
            terminal_font_size: 19.0,
            ..AppSettings::default()
        };
        let mut older_library = Library::default();
        let note = older_library.create_note(None, 1);
        older_library.set_note_body(note, "Earlier edit".into(), 1);
        let mut latest_library = older_library.clone();
        latest_library.set_note_body(note, "Last edit before close".into(), 2);

        queue.save_workspace(1, older).unwrap();
        queue.save_settings(1, older_settings).unwrap();
        queue.save_library(1, older_library).unwrap();
        queue
            .finish(
                Some((2, latest.clone())),
                Some((2, latest_settings.clone())),
                Some((2, latest_library.clone())),
            )
            .unwrap();
        assert_eq!(workspace_repository.load().unwrap().unwrap(), latest);
        assert_eq!(settings_repository.load().unwrap(), latest_settings);
        assert_eq!(library_repository.load().unwrap(), latest_library);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn final_write_reports_storage_errors() {
        let root = std::env::temp_dir().join(format!("vibra-persistence-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let blocker = root.join("not-a-directory");
        std::fs::write(&blocker, b"blocker").unwrap();
        let repository = WorkspaceRepository::at(blocker.join("workspace.json"));
        let settings_repository = SettingsRepository::at(root.join("settings.json"));
        let (queue, results) =
            PersistenceQueue::start(repository, settings_repository, None).unwrap();

        let error = queue
            .finish(Some((1, WorkspaceSnapshot::default())), None, None)
            .unwrap_err();
        assert!(matches!(error, FinishError::Save(message) if message.contains("proyectos")));
        assert!(matches!(
            results.try_recv(),
            Ok(SaveResult::Workspace {
                generation: 1,
                error: Some(_)
            })
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn close_waits_for_a_delayed_disk_write() {
        let root = std::env::temp_dir().join(format!("vibra-persistence-{}", Uuid::new_v4()));
        let path = root.join("workspace.json");
        let held_path = path.clone();
        let (held_sender, held_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let holder = thread::spawn(move || {
            with_exclusive_file_lock(&held_path, || {
                held_sender.send(()).unwrap();
                release_receiver.recv().unwrap();
                Ok(())
            })
            .unwrap();
        });
        held_receiver.recv().unwrap();

        let workspace_repository = WorkspaceRepository::at(&path);
        let settings_repository = SettingsRepository::at(root.join("settings.json"));
        let (queue, _) =
            PersistenceQueue::start(workspace_repository, settings_repository, None).unwrap();
        let finish = thread::spawn(move || {
            queue.finish(Some((1, WorkspaceSnapshot::default())), None, None)
        });
        thread::sleep(Duration::from_millis(2100));
        assert!(
            !finish.is_finished(),
            "close returned before storage was available"
        );
        release_sender.send(()).unwrap();
        holder.join().unwrap();
        finish.join().unwrap().unwrap();
        assert!(path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
