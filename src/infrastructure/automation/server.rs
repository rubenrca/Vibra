use anyhow::{Context, Result, bail};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use super::types::*;

pub struct AutomationServer {
    path: PathBuf,
    receiver: async_channel::Receiver<AutomationIncoming>,
    stopped: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl AutomationServer {
    pub fn start() -> Result<Self> {
        let directory = automation_directory();
        prepare_automation_directory(&directory)?;
        // A crashed process may leave its socket behind. A random name lets a
        // new server start without ever unlinking another process's socket.
        let path = directory.join(format!("{}.sock", uuid::Uuid::new_v4().simple()));
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("no se pudo abrir {}", path.display()))?;
        if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .and_then(|_| listener.set_nonblocking(true))
        {
            let _ = fs::remove_file(&path);
            return Err(error).with_context(|| format!("no se pudo preparar {}", path.display()));
        }
        let (sender, receiver) = async_channel::bounded(AUTOMATION_QUEUE_CAPACITY);
        let stopped = Arc::new(AtomicBool::new(false));
        let thread_stopped = stopped.clone();
        let client_threads = Arc::new(AtomicU64::new(0));
        let thread = thread::Builder::new()
            .name("vibra-automation".into())
            .spawn(move || {
                loop {
                    if thread_stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(25));
                            continue;
                        }
                        Err(_) => {
                            thread::sleep(Duration::from_millis(25));
                            continue;
                        }
                    };
                    if stream.set_nonblocking(false).is_err() {
                        continue;
                    }
                    let sender = sender.clone();
                    let live = client_threads.fetch_add(1, Ordering::AcqRel);
                    if live >= AUTOMATION_MAX_CLIENT_THREADS as u64 {
                        client_threads.fetch_sub(1, Ordering::AcqRel);
                        let _ = write_automation_response(
                            stream,
                            &AutomationResponse::failure(
                                "demasiadas solicitudes de automatización",
                            ),
                        );
                        continue;
                    }
                    let active_threads = client_threads.clone();
                    if thread::Builder::new()
                        .name("vibra-automation-client".into())
                        .spawn(move || {
                            handle_connection(stream, &sender);
                            active_threads.fetch_sub(1, Ordering::AcqRel);
                        })
                        .is_err()
                    {
                        client_threads.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            })
            .inspect_err(|_| {
                let _ = fs::remove_file(&path);
            })?;
        Ok(Self {
            path,
            receiver,
            stopped,
            thread: Some(thread),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn receiver(&self) -> async_channel::Receiver<AutomationIncoming> {
        self.receiver.clone()
    }
}

impl Drop for AutomationServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

fn handle_connection(mut stream: UnixStream, sender: &async_channel::Sender<AutomationIncoming>) {
    let mut request = String::new();
    let parsed = stream
        .set_read_timeout(Some(AUTOMATION_IO_TIMEOUT))
        .and_then(|_| stream.set_write_timeout(Some(AUTOMATION_IO_TIMEOUT)))
        .map_err(anyhow::Error::from)
        .and_then(|_| {
            Read::by_ref(&mut stream)
                .take(MAX_AUTOMATION_REQUEST_BYTES + 1)
                .read_to_string(&mut request)?;
            if request.len() as u64 > MAX_AUTOMATION_REQUEST_BYTES {
                bail!("la solicitud supera 1 MiB");
            }
            serde_json::from_str::<AutomationEnvelope>(&request).map_err(Into::into)
        });
    let response = match parsed {
        Ok(envelope) => {
            let (response_tx, response_rx) = mpsc::channel();
            match enqueue_automation_request(
                sender,
                AutomationIncoming {
                    envelope,
                    response: response_tx,
                },
            ) {
                Err(error) => AutomationResponse::failure(error),
                Ok(()) => response_rx
                    .recv_timeout(AUTOMATION_IO_TIMEOUT)
                    .unwrap_or_else(|_| AutomationResponse::failure("la UI no respondió a tiempo")),
            }
        }
        Err(error) => AutomationResponse::failure(format!("solicitud inválida: {error}")),
    };
    let _ = write_automation_response(stream, &response);
}

fn write_automation_response(mut stream: UnixStream, response: &AutomationResponse) -> Result<()> {
    stream.write_all(&serde_json::to_vec(response)?)?;
    Ok(())
}

/// Fail closed when the UI queue is full instead of growing without bound.
pub(super) fn enqueue_automation_request(
    sender: &async_channel::Sender<AutomationIncoming>,
    incoming: AutomationIncoming,
) -> Result<(), &'static str> {
    sender.try_send(incoming).map_err(|error| match error {
        async_channel::TrySendError::Full(_) => "demasiadas solicitudes de automatización",
        async_channel::TrySendError::Closed(_) => "Vibra se está cerrando",
    })
}

fn automation_directory() -> PathBuf {
    let user = unsafe { libc::geteuid() };
    std::env::temp_dir().join(format!("vibra-{user}"))
}

fn prepare_automation_directory(directory: &Path) -> Result<()> {
    fs::create_dir_all(directory)
        .with_context(|| format!("no se pudo crear {}", directory.display()))?;
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.file_type().is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        bail!(
            "{} no es un directorio privado del usuario",
            directory.display()
        );
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn automation_directory_rejects_symlinks() {
        let root = std::env::temp_dir().join(format!("vibra-server-{}", uuid::Uuid::new_v4()));
        let real = root.join("real");
        let link = root.join("link");
        fs::create_dir_all(&real).unwrap();
        symlink(&real, &link).unwrap();

        assert!(prepare_automation_directory(&link).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
