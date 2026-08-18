use anyhow::{Context, Result, bail};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use super::*;

pub struct AutomationServer {
    path: PathBuf,
    receiver: async_channel::Receiver<AutomationIncoming>,
    stopped: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl AutomationServer {
    pub fn start() -> Result<Self> {
        let directory = automation_directory();
        fs::create_dir_all(&directory)
            .with_context(|| format!("no se pudo crear {}", directory.display()))?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let server_id = NEXT_AUTOMATION_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("{}-{server_id}.sock", std::process::id()));
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("no se pudo abrir {}", path.display()))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let (sender, receiver) = async_channel::bounded(AUTOMATION_QUEUE_CAPACITY);
        let stopped = Arc::new(AtomicBool::new(false));
        let thread_stopped = stopped.clone();
        let client_threads = Arc::new(AtomicU64::new(0));
        let thread = thread::Builder::new()
            .name("vibra-automation".into())
            .spawn(move || {
                for connection in listener.incoming() {
                    if thread_stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = connection else {
                        continue;
                    };
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
                    let client_threads = client_threads.clone();
                    let _ = thread::Builder::new()
                        .name("vibra-automation-client".into())
                        .spawn(move || {
                            handle_connection(stream, &sender);
                            client_threads.fetch_sub(1, Ordering::AcqRel);
                        });
                }
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
        let _ = UnixStream::connect(&self.path);
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
                    .recv_timeout(AUTOMATION_RESPONSE_TIMEOUT)
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
pub fn enqueue_automation_request(
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
