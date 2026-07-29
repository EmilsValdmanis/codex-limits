use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

use crate::model::{LimitWindow, NormalizeError, normalize_rate_limits};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const STDERR_LIMIT: usize = 8 * 1024;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AppServerError {
    #[error("CODEX_HOME is invalid: {0}")]
    InvalidHome(String),
    #[error("the Codex executable was not found: {0}")]
    CodexNotFound(String),
    #[error("Codex is not signed in for this home")]
    NotAuthenticated,
    #[error("this Codex app-server does not support rate-limit reads")]
    MethodUnavailable,
    #[error("the Codex app-server timed out")]
    Timeout,
    #[error("the Codex app-server exited unexpectedly: {0}")]
    ProcessExited(String),
    #[error("invalid app-server protocol response: {0}")]
    Protocol(String),
    #[error("the app-server did not return rate-limit data")]
    EmptyRateLimits,
    #[error("Codex could not fetch account data: {0}")]
    Upstream(String),
}

impl AppServerError {
    pub fn tile_label(&self) -> &'static str {
        match self {
            Self::InvalidHome(_) => "Bad home",
            Self::CodexNotFound(_) => "Codex missing",
            Self::NotAuthenticated => "Sign in",
            Self::MethodUnavailable => "Update Codex",
            Self::Timeout | Self::Upstream(_) => "Offline",
            Self::ProcessExited(_) | Self::Protocol(_) | Self::EmptyRateLimits => "Unavailable",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountIdentity {
    pub codex_home: String,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub signed_in: bool,
}

#[derive(Clone, Debug)]
pub struct AppServerClient {
    executable: String,
    codex_home: PathBuf,
}

impl AppServerClient {
    pub fn new(executable: impl Into<String>, codex_home: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            codex_home: codex_home.into(),
        }
    }

    pub async fn fetch_limits(&self) -> Result<Vec<LimitWindow>, AppServerError> {
        let value = self.request("account/rateLimits/read", 2, None).await?;
        normalize_rate_limits(&value).map_err(|error| match error {
            NormalizeError::MissingSnapshot => AppServerError::EmptyRateLimits,
            NormalizeError::InvalidResponse(message) => AppServerError::Protocol(message),
        })
    }

    pub async fn fetch_identity(&self) -> Result<AccountIdentity, AppServerError> {
        let value = self
            .request("account/read", 3, Some(json!({ "refreshToken": false })))
            .await?;
        let result = value.get("result").unwrap_or(&value);
        let account = result.get("account").filter(|account| !account.is_null());

        Ok(AccountIdentity {
            codex_home: self.codex_home.to_string_lossy().into_owned(),
            email: account
                .and_then(|account| account.get("email"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            plan_type: account
                .and_then(|account| account.get("planType"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            signed_in: account.is_some(),
        })
    }

    async fn request(
        &self,
        method: &str,
        id: u64,
        params: Option<Value>,
    ) -> Result<Value, AppServerError> {
        self.request_with_timeout(method, id, params, REQUEST_TIMEOUT)
            .await
    }

    async fn request_with_timeout(
        &self,
        method: &str,
        id: u64,
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
        let stderr_task = tokio::spawn(read_stderr_tail(stderr));

        let result = timeout(request_timeout, exchange(&mut child, method, id, params)).await;

        let response = match result {
            Ok(response) => response,
            Err(_) => Err(AppServerError::Timeout),
        };

        cleanup_child(&mut child).await;
        let stderr = stderr_task.await.unwrap_or_default();

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

pub fn validate_home(path: &Path) -> Result<(), AppServerError> {
    if !path.is_dir() {
        return Err(AppServerError::InvalidHome(
            path.to_string_lossy().into_owned(),
        ));
    }
    Ok(())
}

async fn exchange(
    child: &mut Child,
    method: &str,
    id: u64,
    params: Option<Value>,
) -> Result<Value, AppServerError> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| AppServerError::Protocol("app-server stdin was unavailable".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppServerError::Protocol("app-server stdout was unavailable".into()))?;
    let mut lines = BufReader::new(stdout).lines();

    send(
        &mut stdin,
        &json!({
            "method": "initialize",
            "id": 0,
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

    let initialization = wait_for_id(&mut lines, 0).await?;
    ensure_success(&initialization)?;

    send(
        &mut stdin,
        &json!({ "method": "initialized", "params": {} }),
    )
    .await?;

    let mut request = json!({ "method": method, "id": id });
    if let Some(params) = params {
        request["params"] = params;
    }
    send(&mut stdin, &request).await?;

    let response = wait_for_id(&mut lines, id).await?;
    ensure_success(&response)?;
    drop(stdin);
    Ok(response)
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

async fn wait_for_id(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    expected: u64,
) -> Result<Value, AppServerError> {
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| AppServerError::Protocol(error.to_string()))?
    {
        let value: Value = serde_json::from_str(&line)
            .map_err(|error| AppServerError::Protocol(error.to_string()))?;
        if value.get("id").and_then(Value::as_u64) == Some(expected) {
            return Ok(value);
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn fake_codex(directory: &Path) -> PathBuf {
        let executable = directory.join("fake-codex");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
IFS= read -r initialize
printf '%s\n' '{"id":0,"result":{"userAgent":"fake"}}'
IFS= read -r initialized
IFS= read -r request
case "$request" in
  *account/rateLimits/read*)
    printf '%s\n' '{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":12,"windowDurationMins":300},"secondary":{"usedPercent":34,"windowDurationMins":10080}}}}'
    ;;
  *account/read*)
    printf '%s\n' '{"id":3,"result":{"account":{"type":"chatgpt","email":"test@example.com","planType":"plus"},"requiresOpenaiAuth":true}}'
    ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        executable
    }

    fn hanging_codex(directory: &Path) -> (PathBuf, PathBuf) {
        let executable = directory.join("hanging-codex");
        let pid_file = directory.join("child.pid");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$$\" > '{}'\nexec sleep 30\n",
                pid_file.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        (executable, pid_file)
    }

    #[tokio::test]
    async fn completes_the_initialize_and_rate_limit_exchange() {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_codex(directory.path());
        let client = AppServerClient::new(executable.to_string_lossy(), directory.path());

        let limits = client.fetch_limits().await.unwrap();
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].remaining_percent, 88);
        assert_eq!(limits[1].remaining_percent, 66);
    }

    #[tokio::test]
    async fn reads_account_identity_without_persisting_it() {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_codex(directory.path());
        let client = AppServerClient::new(executable.to_string_lossy(), directory.path());

        let identity = client.fetch_identity().await.unwrap();
        assert!(identity.signed_in);
        assert_eq!(identity.email.as_deref(), Some("test@example.com"));
        assert_eq!(identity.plan_type.as_deref(), Some("plus"));
    }

    #[tokio::test]
    async fn keeps_only_the_bounded_stderr_tail() {
        let (mut writer, reader) = tokio::io::duplex(16 * 1024);
        let writer_task = tokio::spawn(async move {
            writer.write_all(b"discard-this-prefix\n").await.unwrap();
            writer.write_all(&vec![b'x'; STDERR_LIMIT]).await.unwrap();
            writer.write_all(b"\nkeep-this-suffix").await.unwrap();
        });

        let tail = read_stderr_tail(reader).await;
        writer_task.await.unwrap();

        assert!(tail.len() <= STDERR_LIMIT);
        assert!(!tail.contains("discard-this-prefix"));
        assert!(tail.ends_with("keep-this-suffix"));
    }

    #[tokio::test]
    async fn times_out_and_reaps_the_app_server_child() {
        let directory = tempfile::tempdir().unwrap();
        let (executable, pid_file) = hanging_codex(directory.path());
        let client = AppServerClient::new(executable.to_string_lossy(), directory.path());

        let error = client
            .request_with_timeout(
                "account/rateLimits/read",
                2,
                None,
                Duration::from_millis(300),
            )
            .await
            .unwrap_err();
        assert_eq!(error, AppServerError::Timeout);

        let pid = std::fs::read_to_string(pid_file).unwrap();
        assert!(!Path::new("/proc").join(pid.trim()).exists());
    }

    #[tokio::test]
    #[ignore = "requires a signed-in local CODEX_HOME and real Codex CLI"]
    async fn reads_live_rate_limits() {
        let home = std::env::var("CODEX_HOME").expect("set CODEX_HOME for this test");
        let limits = AppServerClient::new("codex", home)
            .fetch_limits()
            .await
            .unwrap();
        assert!(limits.len() <= 2);
    }
}
