use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

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
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_reader: Option<JoinHandle<io::Result<()>>>,
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
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_reader = child
            .stderr
            .take()
            .map(|stream| spawn_stderr_reader(stream, Arc::clone(&stderr)));
        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            stderr,
            stderr_reader,
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
        self.stdin.write_all(&encoded)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        let mut line = Vec::new();
        for _ in 0..=16 * 1024 * 1024 {
            let mut byte = [0_u8; 1];
            if self.stdout.read(&mut byte)? == 0 {
                return Err(self.worker_exit_error());
            }
            line.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        if line.len() > 16 * 1024 * 1024 || !line.ends_with(b"\n") {
            return Err(io::Error::other("worker response exceeds 16 MiB limit"));
        }
        let response: Value =
            serde_json::from_slice(&line[..line.len() - 1]).map_err(io::Error::other)?;
        if response.get("protocol_version").and_then(Value::as_u64) != Some(1)
            || response.get("request_id").and_then(Value::as_u64) != Some(request_id)
        {
            return Err(io::Error::other("worker response identity mismatch"));
        }
        Ok(response)
    }

    /// Terminate the worker during trusted lifecycle teardown.
    pub fn terminate(&mut self) -> io::Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        self.child.wait().map(|_| ())
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
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

const MAX_WORKER_STDERR_BYTES: usize = 64 * 1024;

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
    use super::PodmanWorkerSpec;
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
}
