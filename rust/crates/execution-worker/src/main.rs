use std::io::{self, BufRead, Write};

use runtime::{execute_bash, BashCommandInput, FilesystemCapability, GrepSearchInput};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    ReadFile {
        path: String,
        offset: Option<usize>,
        limit: Option<usize>,
    },
    WriteFile {
        path: String,
        content: String,
    },
    EditFile {
        path: String,
        old_string: String,
        new_string: String,
        replace_all: bool,
    },
    Glob {
        pattern: String,
        path: Option<String>,
    },
    Grep {
        input: GrepSearchInput,
    },
    RunCommand {
        command: String,
        timeout: Option<u64>,
    },
}

#[derive(Debug, Serialize)]
struct Response {
    protocol_version: u32,
    request_id: u64,
    ok: bool,
    result: Option<Value>,
    error: Option<String>,
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let workspace = std::env::current_dir()?;
    let filesystem = FilesystemCapability::workspace(workspace);

    for line in stdin.lock().lines() {
        let response = match line {
            Ok(line) => handle_request(&filesystem, &line),
            Err(error) => Response {
                protocol_version: 1,
                request_id: 0,
                ok: false,
                result: None,
                error: Some(error.to_string()),
            },
        };
        serde_json::to_writer(&mut stdout, &response)
            .map_err(|error| io::Error::other(error.to_string()))?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
}

fn handle_request(filesystem: &FilesystemCapability, line: &str) -> Response {
    let envelope = serde_json::from_str::<Value>(line);
    let (protocol_version, request_id, result) = match envelope {
        Ok(value) => {
            let protocol_version = value
                .get("protocol_version")
                .and_then(Value::as_u64)
                .and_then(|version| u32::try_from(version).ok())
                .unwrap_or(0);
            let request_id = value.get("request_id").and_then(Value::as_u64).unwrap_or(0);
            if protocol_version == 1 {
                let request = serde_json::from_value::<Request>(value)
                    .map_err(|error| error.to_string())
                    .and_then(|request| execute_request(filesystem, request));
                (protocol_version, request_id, request)
            } else {
                (
                    protocol_version,
                    request_id,
                    Err(String::from("unsupported worker protocol version")),
                )
            }
        }
        Err(error) => (1, 0, Err(error.to_string())),
    };
    let result = result
        .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
        .map(bound_worker_result);
    match result {
        Ok(result) => Response {
            protocol_version,
            request_id,
            ok: true,
            result: Some(result),
            error: None,
        },
        Err(error) => Response {
            protocol_version,
            request_id,
            ok: false,
            result: None,
            error: Some(error),
        },
    }
}

const MAX_INLINE_READ_BYTES: usize = 512 * 1024;

/// Keep repository reads useful without allowing one source file to become an
/// unbounded IPC/context payload. The writer can request the next line range
/// using the returned offset.
fn bound_worker_result(mut result: Value) -> Value {
    let Some(file) = result.get_mut("file").and_then(Value::as_object_mut) else {
        return result;
    };
    let Some(content) = file
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return result;
    };
    if content.len() <= MAX_INLINE_READ_BYTES {
        return result;
    }

    let mut end = content
        .char_indices()
        .take_while(|(index, _)| *index <= MAX_INLINE_READ_BYTES)
        .map(|(index, _)| index)
        .last()
        .unwrap_or(0);
    if let Some(newline) = content[..end].rfind('\n') {
        end = newline + 1;
    }
    if end == 0 {
        end = content
            .char_indices()
            .take_while(|(index, _)| *index <= MAX_INLINE_READ_BYTES)
            .map(|(index, _)| index)
            .last()
            .unwrap_or(0);
    }
    let returned = &content[..end];
    let total_bytes = content.len();
    let returned_lines = returned.bytes().filter(|byte| *byte == b'\n').count();
    let start_line = file.get("startLine").and_then(Value::as_u64).unwrap_or(1);
    if let Some(content_slot) = file.get_mut("content") {
        *content_slot = Value::String(returned.to_string());
    }
    file.insert("truncated".to_string(), Value::Bool(true));
    file.insert("returnedBytes".to_string(), Value::from(end as u64));
    file.insert("totalBytes".to_string(), Value::from(total_bytes as u64));
    file.insert(
        "nextOffset".to_string(),
        Value::from(
            start_line
                .saturating_sub(1)
                .saturating_add(returned_lines as u64),
        ),
    );
    result
}

fn execute_request(filesystem: &FilesystemCapability, request: Request) -> Result<Value, String> {
    match request {
        Request::ReadFile {
            path,
            offset,
            limit,
        } => to_value(
            filesystem
                .read_file(&path, offset, limit)
                .map_err(error_text)?,
        ),
        Request::WriteFile { path, content } => {
            to_value(filesystem.write_file(&path, &content).map_err(error_text)?)
        }
        Request::EditFile {
            path,
            old_string,
            new_string,
            replace_all,
        } => to_value(
            filesystem
                .edit_file(&path, &old_string, &new_string, replace_all)
                .map_err(error_text)?,
        ),
        Request::Glob { pattern, path } => to_value(
            filesystem
                .glob_search(&pattern, path.as_deref())
                .map_err(error_text)?,
        ),
        Request::Grep { input } => to_value(filesystem.grep_search(&input).map_err(error_text)?),
        Request::RunCommand { command, timeout } => to_value(
            execute_bash(BashCommandInput {
                command,
                timeout,
                description: None,
                run_in_background: Some(false),
                dangerously_disable_sandbox: None,
                namespace_restrictions: None,
                isolate_network: None,
                filesystem_mode: None,
                allowed_mounts: None,
            })
            .map_err(error_text)?,
        ),
    }
}

fn to_value<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

#[allow(clippy::needless_pass_by_value)]
fn error_text(error: io::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::bound_worker_result;
    use serde_json::json;

    #[test]
    fn large_read_result_is_bounded_with_continuation_metadata() {
        let content = "0123456789abcdef\n".repeat(40_000);
        let result = bound_worker_result(json!({
            "file": {
                "content": content,
                "startLine": 1,
                "totalLines": 40_000
            }
        }));
        let file = &result["file"];
        assert_eq!(file["truncated"], true);
        assert!(file["content"].as_str().unwrap().len() <= 512 * 1024);
        assert!(file["returnedBytes"].as_u64().unwrap() > 0);
        assert!(file["totalBytes"].as_u64().unwrap() > file["returnedBytes"].as_u64().unwrap());
        assert!(file["nextOffset"].as_u64().unwrap() > 0);
    }

    #[test]
    fn small_read_result_is_unchanged() {
        let result = json!({
            "file": {
                "content": "small\n",
                "startLine": 1,
                "totalLines": 1
            }
        });
        assert_eq!(bound_worker_result(result.clone()), result);
    }
}
