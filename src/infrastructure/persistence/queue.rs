//! Ordered persistence off the GPUI thread. The final message carries the
//! latest state, so a delayed write cannot replace it during window teardown.

use std::collections::BTreeMap;
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DocumentKind {
    Workspace,
    Settings,
    Library,
}

impl DocumentKind {
    fn label(self) -> &'static str {
        match self {
            Self::Workspace => "projects",
            Self::Settings => "settings",
            Self::Library => "notes and automations",
        }
    }
}

enum Document {
    Workspace(WorkspaceSnapshot),
    Settings(Box<AppSettings>),
    Library(Library),
}

impl Document {
    fn kind(&self) -> DocumentKind {
        match self {
            Self::Workspace(_) => DocumentKind::Workspace,
            Self::Settings(_) => DocumentKind::Settings,
            Self::Library(_) => DocumentKind::Library,
        }
    }
}

pub(crate) struct SaveResult {
    pub kind: DocumentKind,
    pub generation: u64,
    pub error: Option<String>,
}

#[derive(Debug)]
pub(crate) enum FinishError {
    Unavailable,
    Save(String),
}

enum Command {
    Wake,
    Finish {
        state: PendingWrites,
        completed: mpsc::Sender<Vec<String>>,
    },
    #[cfg(test)]
    Barrier(mpsc::Sender<()>),
    Stop,
}

// One latest snapshot per document, written in workspace/settings/library order.
type PendingWrites = BTreeMap<DocumentKind, (u64, Document)>;

struct Repositories {
    workspace: WorkspaceRepository,
    settings: SettingsRepository,
    library: Option<LibraryRepository>,
}

pub(crate) struct PersistenceQueue {
    commands: mpsc::Sender<Command>,
    pending: Arc<Mutex<PendingWrites>>,
    wake_queued: Arc<AtomicBool>,
    _worker: thread::JoinHandle<()>,
}

impl PersistenceQueue {
    pub fn start(
        workspace: WorkspaceRepository,
        settings: SettingsRepository,
        library: Option<LibraryRepository>,
    ) -> io::Result<(Self, Receiver<SaveResult>)> {
        let (commands, receiver) = mpsc::channel();
        let (results, result_receiver) = async_channel::unbounded();
        let pending = Arc::new(Mutex::new(PendingWrites::default()));
        let worker_pending = pending.clone();
        let wake_queued = Arc::new(AtomicBool::new(false));
        let worker_wake_queued = wake_queued.clone();
        let repositories = Repositories {
            workspace,
            settings,
            library,
        };
        let worker = thread::Builder::new()
            .name("vibra-persistence".into())
            .spawn(move || {
                run(
                    receiver,
                    worker_pending,
                    worker_wake_queued,
                    results,
                    repositories,
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
        self.enqueue(generation, Document::Workspace(snapshot))
    }

    pub fn save_settings(&self, generation: u64, settings: AppSettings) -> Result<(), String> {
        self.enqueue(generation, Document::Settings(Box::new(settings)))
    }

    pub fn save_library(&self, generation: u64, library: Library) -> Result<(), String> {
        self.enqueue(generation, Document::Library(library))
    }

    fn enqueue(&self, generation: u64, document: Document) -> Result<(), String> {
        let kind = document.kind();
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(kind, (generation, document));
        if !self.wake_queued.swap(true, Ordering::AcqRel)
            && self.commands.send(Command::Wake).is_err()
        {
            self.wake_queued.store(false, Ordering::Release);
            return Err(format!("{} saving is no longer available", kind.label()));
        }
        Ok(())
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
                state: final_writes(workspace, settings, library),
                completed,
            })
            .map_err(|_| FinishError::Unavailable)?;
        finish_result(reply.recv().map_err(|_| FinishError::Unavailable)?)
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

/// Emergency path when the queue could not start or has stopped. Uses the same
/// save and error handling as the worker, completing before the process exits.
pub(crate) fn save_final_blocking(
    workspace_repository: WorkspaceRepository,
    settings_repository: SettingsRepository,
    library_repository: Option<LibraryRepository>,
    workspace: Option<WorkspaceSnapshot>,
    settings: Option<AppSettings>,
    library: Option<Library>,
) -> Result<(), FinishError> {
    let repositories = Repositories {
        workspace: workspace_repository,
        settings: settings_repository,
        library: library_repository,
    };
    let writes = final_writes(
        workspace.map(|snapshot| (0, snapshot)),
        settings.map(|settings| (0, settings)),
        library.map(|library| (0, library)),
    );
    finish_result(persist_batch(writes, &repositories, None))
}

fn final_writes(
    workspace: Option<(u64, WorkspaceSnapshot)>,
    settings: Option<(u64, AppSettings)>,
    library: Option<(u64, Library)>,
) -> PendingWrites {
    [
        workspace.map(|(generation, snapshot)| (generation, Document::Workspace(snapshot))),
        settings.map(|(generation, settings)| (generation, Document::Settings(Box::new(settings)))),
        library.map(|(generation, library)| (generation, Document::Library(library))),
    ]
    .into_iter()
    .flatten()
    .map(|(generation, document)| (document.kind(), (generation, document)))
    .collect()
}

fn finish_result(errors: Vec<String>) -> Result<(), FinishError> {
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
    repositories: Repositories,
) {
    while let Ok(mut command) = commands.recv() {
        if matches!(command, Command::Wake) {
            // Clear before taking pending state so a concurrent edit can wake us again.
            wake_queued.store(false, Ordering::Release);
        }
        let mut writes = std::mem::take(
            &mut *pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        if let Command::Finish { state, .. } = &mut command {
            writes.extend(std::mem::take(state));
        }
        let errors = persist_batch(writes, &repositories, Some(&results));
        match command {
            Command::Wake => {}
            Command::Finish { completed, .. } => {
                let _ = completed.send(errors);
                break;
            }
            #[cfg(test)]
            Command::Barrier(completed) => {
                let _ = completed.send(());
            }
            Command::Stop => break,
        }
    }
}

fn persist_batch(
    writes: PendingWrites,
    repositories: &Repositories,
    results: Option<&async_channel::Sender<SaveResult>>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for (kind, (generation, document)) in writes {
        let saved = match document {
            Document::Workspace(snapshot) => repositories.workspace.save(&snapshot).map(|_| ()),
            Document::Settings(settings) => repositories.settings.save(&settings),
            Document::Library(library) => repositories
                .library
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("saving is unavailable"))
                .and_then(|repository| repository.save(&library)),
        };
        let error = saved
            .err()
            .map(|error| format!("Could not save {}: {error}", kind.label()));
        if let Some(error) = &error {
            errors.push(error.clone());
        }
        if let Some(results) = results {
            let _ = results.try_send(SaveResult {
                kind,
                generation,
                error,
            });
        }
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
        assert!(matches!(error, FinishError::Save(message) if message.contains("projects")));
        assert!(matches!(
            results.try_recv(),
            Ok(SaveResult {
                kind: DocumentKind::Workspace,
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
