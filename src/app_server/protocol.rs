use std::io::ErrorKind;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinSet;
use tokio::time::timeout;

use super::{AppServerClient, AppServerError, validate_home};

const INITIALIZE_ID: u64 = 0;
const REQUEST_ID: u64 = 1;
const STDERR_DRAIN_TIMEOUT: Duration = Duration::from_millis(100);
const STDERR_LIMIT: usize = 8 * 1024;

impl AppServerClient {
    pub(super) async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        request_timeout: Duration,
    ) -> Result<Value, AppServerError> {
        validate_home(&self.codex_home)?;

        let mut child = Command::new(&self.executable)
            .args(["app-server", "--stdio"])
            .env("CODEX_HOME", &self.codex_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                if error.kind() == ErrorKind::NotFound {
                    AppServerError::CodexNotFound(self.executable.clone())
                } else {
                    AppServerError::ProcessExited(error.to_string())
                }
            })?;

        let stderr = child.stderr.take().expect("piped stderr");
        let mut stderr_tasks = JoinSet::new();
        stderr_tasks.spawn(read_stderr_tail(stderr));

        let response = timeout(request_timeout, exchange(&mut child, method, params))
            .await
            .unwrap_or(Err(AppServerError::Timeout));

        cleanup_child(&mut child).await;
        // A descendant can retain stderr after the app-server exits. Diagnostics
        // must not keep a completed or timed-out request waiting indefinitely.
        let stderr = match timeout(STDERR_DRAIN_TIMEOUT, stderr_tasks.join_next()).await {
            Ok(Some(Ok(stderr))) => stderr,
            _ => String::new(),
        };

        response.map_err(|error| match error {
            AppServerError::ProcessExited(message) if !stderr.is_empty() => {
                AppServerError::ProcessExited(format!("{message}: {stderr}"))
            }
            other => other,
        })
    }
}

async fn read_stderr_tail(mut stderr: impl tokio::io::AsyncRead + Unpin) -> String {
    let mut tail = Vec::with_capacity(STDERR_LIMIT);
    let mut chunk = [0_u8; 1_024];

    loop {
        let count = match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };

        let overflow = tail
            .len()
            .saturating_add(count)
            .saturating_sub(STDERR_LIMIT);
        if overflow > 0 {
            tail.drain(..overflow);
        }
        tail.extend_from_slice(&chunk[..count]);
    }

    String::from_utf8_lossy(&tail).trim().to_owned()
}

async fn exchange(
    child: &mut Child,
    method: &str,
    params: Option<Value>,
) -> Result<Value, AppServerError> {
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();

    send(
        &mut stdin,
        &json!({
            "method": "initialize",
            "id": INITIALIZE_ID,
            "params": {
                "clientInfo": {
                    "name": "codex_limits_opendeck",
                    "title": "Codex Limits for OpenDeck",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
    )
    .await?;

    read_result(&mut lines, INITIALIZE_ID).await?;

    send(
        &mut stdin,
        &json!({ "method": "initialized", "params": {} }),
    )
    .await?;

    let mut request = json!({ "method": method, "id": REQUEST_ID });
    if let Some(params) = params {
        request["params"] = params;
    }
    send(&mut stdin, &request).await?;

    read_result(&mut lines, REQUEST_ID).await
}

async fn send(stdin: &mut tokio::process::ChildStdin, value: &Value) -> Result<(), AppServerError> {
    let mut bytes =
        serde_json::to_vec(value).map_err(|error| AppServerError::Protocol(error.to_string()))?;
    bytes.push(b'\n');
    stdin
        .write_all(&bytes)
        .await
        .map_err(|error| AppServerError::ProcessExited(error.to_string()))?;
    stdin
        .flush()
        .await
        .map_err(|error| AppServerError::ProcessExited(error.to_string()))
}

async fn read_result(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    expected: u64,
) -> Result<Value, AppServerError> {
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| AppServerError::Protocol(error.to_string()))?
    {
        let mut value: Value = serde_json::from_str(&line)
            .map_err(|error| AppServerError::Protocol(error.to_string()))?;
        if value.get("id").and_then(Value::as_u64) == Some(expected) {
            ensure_success(&value)?;
            return Ok(match value.get_mut("result") {
                Some(result) => result.take(),
                None => value,
            });
        }
    }

    Err(AppServerError::ProcessExited(
        "stdout closed before a response arrived".into(),
    ))
}

fn ensure_success(value: &Value) -> Result<(), AppServerError> {
    let Some(error) = value.get("error") else {
        return Ok(());
    };

    let code = error.get("code").and_then(Value::as_i64);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown app-server error");
    let lower = message.to_ascii_lowercase();

    if code == Some(-32601) || lower.contains("method not found") {
        return Err(AppServerError::MethodUnavailable);
    }
    if lower.contains("unauthorized")
        || lower.contains("not logged")
        || lower.contains("authentication")
        || lower.contains("sign in")
    {
        return Err(AppServerError::NotAuthenticated);
    }

    Err(AppServerError::Upstream(message.to_owned()))
}

async fn cleanup_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill().await;
    }
    let _ = child.wait().await;
}
