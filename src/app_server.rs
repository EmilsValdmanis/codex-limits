mod protocol;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use thiserror::Error;

use crate::settings::TileSettings;
use crate::usage::{NormalizeError, UsageSnapshot, normalize_usage_snapshot};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AppServerError {
    #[error("CODEX_HOME is invalid: select a CODEX_HOME")]
    HomeNotSelected,
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

    pub fn from_settings(settings: &TileSettings) -> Result<Self, AppServerError> {
        let home = settings.codex_home.trim();
        if home.is_empty() {
            return Err(AppServerError::HomeNotSelected);
        }
        let path = match home
            .strip_prefix("~/")
            .or_else(|| (home == "~").then_some(""))
        {
            Some(rest) => PathBuf::from(
                std::env::var_os("HOME")
                    .ok_or_else(|| AppServerError::InvalidHome("HOME is not set".into()))?,
            )
            .join(rest),
            None => PathBuf::from(home),
        };
        let home = canonical_home(&path)?;
        let executable = settings.codex_executable.trim();
        if executable.is_empty() {
            return Err(AppServerError::CodexNotFound(
                "the executable setting is empty".into(),
            ));
        }
        Ok(Self::new(executable, home))
    }

    pub async fn fetch_limits(&self) -> Result<UsageSnapshot, AppServerError> {
        let value = self
            .request("account/rateLimits/read", None, REQUEST_TIMEOUT)
            .await?;
        normalize_usage_snapshot(&value).map_err(|error| match error {
            NormalizeError::MissingSnapshot => AppServerError::EmptyRateLimits,
            NormalizeError::InvalidResponse(message) => AppServerError::Protocol(message),
        })
    }

    pub async fn read_account(&self) -> Result<Value, AppServerError> {
        self.request(
            "account/read",
            Some(json!({ "refreshToken": false })),
            REQUEST_TIMEOUT,
        )
        .await
    }
}

pub fn canonical_home(path: &Path) -> Result<PathBuf, AppServerError> {
    let home = path
        .canonicalize()
        .map_err(|_| AppServerError::InvalidHome(path.display().to_string()))?;
    validate_home(&home)?;
    Ok(home)
}

fn validate_home(path: &Path) -> Result<(), AppServerError> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(AppServerError::InvalidHome(path.display().to_string()))
    }
}

#[cfg(test)]
pub const FAKE_CODEX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake-codex.sh");

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use tokio::time::timeout;

    use super::*;

    #[test]
    fn resolves_settings_to_a_canonical_endpoint() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let alias = directory.path().join("alias");
        fs::create_dir(&home).unwrap();
        symlink(&home, &alias).unwrap();
        let settings = TileSettings {
            codex_executable: " /usr/bin/codex ".into(),
            codex_home: format!(" {} ", alias.display()),
            ..TileSettings::default()
        };
        assert_eq!(
            AppServerClient::from_settings(&settings).unwrap(),
            AppServerClient::new("/usr/bin/codex", home.canonicalize().unwrap())
        );
        fs::remove_dir(&home).unwrap();
        assert!(matches!(
            AppServerClient::from_settings(&settings),
            Err(AppServerError::InvalidHome(_))
        ));
        assert_eq!(
            AppServerClient::from_settings(&TileSettings::default()),
            Err(AppServerError::HomeNotSelected)
        );
    }

    #[tokio::test]
    async fn reads_limits_even_when_a_descendant_keeps_stderr_open() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("inherit-stderr"), "").unwrap();
        let client = AppServerClient::new(FAKE_CODEX, directory.path());
        let snapshot = timeout(Duration::from_secs(1), client.fetch_limits())
            .await
            .expect("an inherited stderr pipe must not block the response")
            .unwrap();
        assert_eq!(
            snapshot
                .windows
                .iter()
                .map(|w| w.remaining_percent)
                .collect::<Vec<_>>(),
            [88, 66]
        );
        assert_eq!(snapshot.reset_credits.unwrap().available_count, 2);
    }

    #[tokio::test]
    async fn reports_the_tail_of_diagnostics_after_an_early_exit() {
        let directory = tempfile::tempdir().unwrap();
        let diagnostic = format!("discard this {} keep this", "x".repeat(10000));
        fs::write(directory.path().join("stderr"), diagnostic).unwrap();
        let client = AppServerClient::new(FAKE_CODEX, directory.path());
        let Err(AppServerError::ProcessExited(message)) = client.fetch_limits().await else {
            panic!("an early exit should include stderr");
        };
        assert!(!message.contains("discard this"));
        assert!(message.ends_with("keep this"));
        assert_eq!(message.split_once(": ").unwrap().1.len(), 8 * 1024);
    }

    #[tokio::test]
    async fn reaps_the_child_on_timeout_even_when_a_descendant_inherits_stderr() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("inherit-stderr"), "").unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(directory.path().join("release"))
                .status()
                .unwrap()
                .success()
        );
        let client = AppServerClient::new(FAKE_CODEX, directory.path());
        let error = timeout(
            Duration::from_secs(1),
            client.request("account/rateLimits/read", None, Duration::from_millis(300)),
        )
        .await
        .expect("stderr must not defeat the timeout")
        .unwrap_err();
        assert_eq!(error, AppServerError::Timeout);
        let pid = fs::read_to_string(directory.path().join("requests")).unwrap();
        assert!(!Path::new("/proc").join(pid.trim()).exists());
    }

    #[tokio::test]
    #[ignore = "requires a signed-in local CODEX_HOME and real Codex CLI"]
    async fn reads_live_rate_limits() {
        let home = std::env::var("CODEX_HOME").expect("set CODEX_HOME for this test");
        AppServerClient::new("codex", home)
            .fetch_limits()
            .await
            .unwrap();
    }
}
