use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodmanWorkerSpec {
    pub image: String,
    pub workspace: PathBuf,
    pub worker: String,
}

#[derive(Debug)]
pub struct PodmanWorkerClient {
    child: Child,
    stdin_tx: Option<Sender<PendingWrite>>,
    stdin_writer: Option<JoinHandle<io::Result<()>>>,
    response_rx: Receiver<io::Result<Vec<u8>>>,
    stdout_reader: Option<JoinHandle<io::Result<()>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_reader: Option<JoinHandle<io::Result<()>>>,
    dispatch_timeout: Duration,
    response_timeout: Duration,
    terminated: bool,
    next_request_id: u64,
    last_request_id: Option<u64>,
    last_operation: Option<String>,
}

impl PodmanWorkerClient {
    pub fn spawn(spec: &PodmanWorkerSpec) -> io::Result<Self> {
        spec.validate_workspace().map_err(io::Error::other)?;
        let command = spec.command();
        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("worker stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("worker stdout unavailable"))?;
        let (stdin_tx, stdin_rx) = mpsc::channel();
        let stdin_writer = spawn_stdin_writer(stdin, stdin_rx);
        let (response_tx, response_rx) = mpsc::channel();
        let stdout_reader = spawn_stdout_reader(stdout, response_tx);
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_reader = child
            .stderr
            .take()
            .map(|stream| spawn_stderr_reader(stream, Arc::clone(&stderr)));
        Ok(Self {
            child,
            stdin_tx: Some(stdin_tx),
            stdin_writer: Some(stdin_writer),
            response_rx,
            stdout_reader: Some(stdout_reader),
            stderr,
            stderr_reader,
            dispatch_timeout: worker_dispatch_timeout(),
            response_timeout: worker_response_timeout(),
            terminated: false,
            next_request_id: 1,
            last_request_id: None,
            last_operation: None,
        })
    }

    pub fn request(&mut self, request: &Value) -> io::Result<Value> {
        let request_id = self.next_request_id;
        self.last_request_id = Some(request_id);
        self.last_operation = request
            .get("operation")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if self.terminated {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "isolated worker has been terminated",
            ));
        }
        if self.child.try_wait()?.is_some() {
            return Err(self.worker_exit_error());
        }
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("worker request id exhausted"))?;
        let mut envelope = request.clone();
        let object = envelope
            .as_object_mut()
            .ok_or_else(|| io::Error::other("worker request must be an object"))?;
        object.insert(String::from("protocol_version"), Value::from(1));
        object.insert(String::from("request_id"), Value::from(request_id));
        let encoded = serde_json::to_vec(&envelope).map_err(io::Error::other)?;
        if encoded.len() > 16 * 1024 * 1024 {
            return Err(io::Error::other("worker request exceeds 16 MiB limit"));
        }
        let (write_result_tx, write_result_rx) = mpsc::channel();
        self.stdin_tx
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "worker stdin unavailable"))?
            .send(PendingWrite {
                frame: encoded,
                completion: write_result_tx,
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "worker stdin closed"))?;
        match write_result_rx.recv_timeout(self.dispatch_timeout) {
            Ok(result) => result?,
            Err(RecvTimeoutError::Timeout) => {
                self.terminate_after_failure();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "worker dispatch timeout after {} ms during {} request {}",
                        self.dispatch_timeout.as_millis(),
                        self.last_operation.as_deref().unwrap_or("unknown"),
                        request_id
                    ),
                ));
            }
            Err(RecvTimeoutError::Disconnected) => return Err(self.worker_exit_error()),
        }
        let frame = match self.response_rx.recv_timeout(self.response_timeout) {
            Ok(result) => result?,
            Err(RecvTimeoutError::Timeout) => {
                self.terminate_after_failure();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "worker IPC response timeout after {} ms during {} request {}",
                        self.response_timeout.as_millis(),
                        self.last_operation.as_deref().unwrap_or("unknown"),
                        request_id
                    ),
                ));
            }
            Err(RecvTimeoutError::Disconnected) => return Err(self.worker_exit_error()),
        };
        let response: Value = serde_json::from_slice(&frame).map_err(io::Error::other)?;
        if response.get("protocol_version").and_then(Value::as_u64) != Some(1)
            || response.get("request_id").and_then(Value::as_u64) != Some(request_id)
        {
            return Err(io::Error::other("worker response identity mismatch"));
        }
        Ok(response)
    }

    /// Terminate the worker during trusted lifecycle teardown.
    pub fn terminate(&mut self) -> io::Result<()> {
        self.terminated = true;
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        self.child.wait().map(|_| ())
    }

    fn terminate_after_failure(&mut self) {
        self.terminated = true;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn worker_exit_error(&mut self) -> io::Error {
        let status = self
            .child
            .try_wait()
            .ok()
            .flatten()
            .map_or_else(|| String::from("unknown"), |value| value.to_string());
        let stderr = self.stderr.lock().map_or_else(
            |_| String::from("worker stderr unavailable"),
            |value| String::from_utf8_lossy(&value).into_owned(),
        );
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "worker exited during {} request {} (status: {status}); stderr: {}",
                self.last_operation.as_deref().unwrap_or("unknown"),
                self.last_request_id.unwrap_or(0),
                if stderr.is_empty() {
                    String::from("<empty>")
                } else {
                    stderr
                }
            ),
        )
    }
}

impl Drop for PodmanWorkerClient {
    fn drop(&mut self) {
        self.stdin_tx.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(writer) = self.stdin_writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

const MAX_WORKER_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_WORKER_STDERR_BYTES: usize = 64 * 1024;
const DEFAULT_WORKER_DISPATCH_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_WORKER_RESPONSE_TIMEOUT_MS: u64 = 120_000;

struct PendingWrite {
    frame: Vec<u8>,
    completion: Sender<io::Result<()>>,
}

fn worker_dispatch_timeout() -> Duration {
    timeout_from_env(
        "CLAW_WORKER_DISPATCH_TIMEOUT_MS",
        DEFAULT_WORKER_DISPATCH_TIMEOUT_MS,
    )
}

fn worker_response_timeout() -> Duration {
    timeout_from_env(
        "CLAW_WORKER_RESPONSE_TIMEOUT_MS",
        DEFAULT_WORKER_RESPONSE_TIMEOUT_MS,
    )
}

fn timeout_from_env(name: &str, default_ms: u64) -> Duration {
    let millis = std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_ms);
    Duration::from_millis(millis)
}

fn spawn_stdin_writer(
    mut stdin: ChildStdin,
    requests: Receiver<PendingWrite>,
) -> JoinHandle<io::Result<()>> {
    std::thread::spawn(move || {
        for pending in requests {
            let result = stdin
                .write_all(&pending.frame)
                .and_then(|()| stdin.write_all(b"\n"))
                .and_then(|()| stdin.flush());
            let failed = result.is_err();
            let _ = pending.completion.send(result);
            if failed {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "worker stdin write failed",
                ));
            }
        }
        Ok(())
    })
}

fn spawn_stdout_reader(
    stdout: ChildStdout,
    responses: Sender<io::Result<Vec<u8>>>,
) -> JoinHandle<io::Result<()>> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_frame(&mut reader) {
                Ok(frame) => {
                    if responses.send(Ok(frame)).is_err() {
                        return Ok(());
                    }
                }
                Err(error) => {
                    let message = io::Error::new(error.kind(), error.to_string());
                    let _ = responses.send(Err(message));
                    return Err(error);
                }
            }
        }
    })
}

fn read_frame<R: BufRead>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "worker stdout closed before a complete response frame",
            ));
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if frame.len().saturating_add(newline) > MAX_WORKER_FRAME_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "worker response exceeds 16 MiB limit",
                ));
            }
            frame.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            return Ok(frame);
        }
        if frame.len().saturating_add(available.len()) > MAX_WORKER_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "worker response exceeds 16 MiB limit",
            ));
        }
        frame.extend_from_slice(available);
        let consumed = available.len();
        reader.consume(consumed);
    }
}

fn spawn_stderr_reader(
    mut stderr: ChildStderr,
    output: Arc<Mutex<Vec<u8>>>,
) -> JoinHandle<io::Result<()>> {
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            let count = stderr.read(&mut buffer)?;
            if count == 0 {
                return Ok(());
            }
            if let Ok(mut captured) = output.lock() {
                let remaining = MAX_WORKER_STDERR_BYTES.saturating_sub(captured.len());
                captured.extend_from_slice(&buffer[..count.min(remaining)]);
            }
        }
    })
}

impl PodmanWorkerSpec {
    #[must_use]
    pub fn command(&self) -> Vec<String> {
        vec![
            "podman".into(),
            "run".into(),
            "--rm".into(),
            "--interactive".into(),
            // The runtime image is selected by trusted host configuration.
            // Never replace it with a registry pull during an execution.
            "--pull=never".into(),
            "--network=none".into(),
            "--read-only".into(),
            "--userns=keep-id".into(),
            "--pid=private".into(),
            "--ipc=private".into(),
            "--cap-drop=ALL".into(),
            "--security-opt=no-new-privileges".into(),
            "--pids-limit=512".into(),
            "--tmpfs".into(),
            "/tmp:rw,nosuid,nodev".into(),
            "--tmpfs".into(),
            "/home/worker:rw,nosuid,nodev".into(),
            "--mount".into(),
            format!(
                "type=bind,src={},dst=/workspace/project,rw",
                self.workspace.display()
            ),
            "--workdir".into(),
            "/workspace/project".into(),
            self.image.clone(),
            self.worker.clone(),
        ]
    }

    pub fn validate_workspace(&self) -> Result<(), String> {
        if !self.workspace.is_absolute() {
            return Err(String::from("isolated workspace must be an absolute path"));
        }
        if !Path::new(&self.worker).is_absolute() && self.worker.contains('/') {
            return Err(String::from("worker path must be an image-local command"));
        }
        Ok(())
    }
}

pub fn require_podman() -> Result<(), String> {
    std::process::Command::new("podman")
        .args(["info", "--format", "{{.Host.Security.Rootless}}"])
        .output()
        .map(|output| {
            if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true" {
                Ok(())
            } else {
                Err(String::from(
                    "rootless Podman is unavailable or not configured",
                ))
            }
        })
        .map_err(|error| format!("secure isolation requires rootless Podman: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::{read_frame, PodmanWorkerSpec, MAX_WORKER_FRAME_BYTES};
    use std::io::Cursor;
    use std::path::PathBuf;

    #[test]
    fn command_is_networkless_and_does_not_mount_host_credentials() {
        let command = PodmanWorkerSpec {
            image: crate::DEFAULT_RUNTIME_IMAGE.to_string(),
            workspace: PathBuf::from("/tmp/claw-snapshot"),
            worker: String::from("/usr/local/bin/claw-exec-worker"),
        }
        .command();
        let rendered = command.join(" ");
        for required in [
            "--pull=never",
            "--network=none",
            "--read-only",
            "--cap-drop=ALL",
            "no-new-privileges",
            "/workspace/project",
        ] {
            assert!(rendered.contains(required), "missing {required}");
        }
        for forbidden in [
            "--privileged",
            "--network=host",
            ".ssh",
            ".aws",
            ".gnupg",
            "SSH_AUTH_SOCK",
            "docker.sock",
            "podman.sock",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "forbidden feature {forbidden}"
            );
        }
    }

    #[test]
    fn framed_reader_handles_multiple_and_unicode_frames() {
        let mut reader = Cursor::new("{\"text\":\"first\"}\n{\"text\":\"π\\nsecond\"}\n");
        assert_eq!(read_frame(&mut reader).unwrap(), b"{\"text\":\"first\"}");
        assert_eq!(
            read_frame(&mut reader).unwrap(),
            "{\"text\":\"π\\nsecond\"}".as_bytes()
        );
    }

    #[test]
    fn framed_reader_rejects_an_unterminated_oversized_frame() {
        let mut payload = vec![b'x'; MAX_WORKER_FRAME_BYTES + 1];
        payload.push(b'\n');
        let error = read_frame(&mut Cursor::new(payload)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
