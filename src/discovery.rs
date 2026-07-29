use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tokio::task::JoinSet;

use crate::app_server::{AccountIdentity, AppServerClient};

pub async fn discover_accounts(executable: &str) -> Vec<AccountIdentity> {
    let candidates = candidate_homes();
    let mut tasks = JoinSet::new();

    for home in candidates {
        let executable = executable.to_owned();
        tasks.spawn(async move {
            let fallback = AccountIdentity {
                codex_home: home.to_string_lossy().into_owned(),
                email: None,
                plan_type: None,
                signed_in: false,
            };
            AppServerClient::new(executable, home)
                .fetch_identity()
                .await
                .unwrap_or(fallback)
        });
    }

    let mut accounts = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok(account) = result {
            accounts.push(account);
        }
    }
    accounts.sort_by(|left, right| left.codex_home.cmp(&right.codex_home));
    accounts
}

pub fn candidate_homes() -> Vec<PathBuf> {
    let inherited = std::env::var_os("CODEX_HOME").map(PathBuf::from);
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    candidate_homes_from(inherited, user_home.as_deref())
}

fn candidate_homes_from(inherited: Option<PathBuf>, user_home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = BTreeSet::new();

    if let Some(home) = inherited {
        insert_candidate(&mut candidates, home, false);
    }

    let Some(user_home) = user_home else {
        return candidates.into_iter().collect();
    };

    insert_candidate(&mut candidates, user_home.join(".codex"), false);

    if let Ok(entries) = std::fs::read_dir(user_home) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with(".codex") {
                insert_candidate(&mut candidates, entry.path(), true);
            }
        }
    }

    candidates.into_iter().collect()
}

fn insert_candidate(candidates: &mut BTreeSet<PathBuf>, path: PathBuf, require_state: bool) {
    if !path.is_dir() {
        return;
    }
    if require_state && !has_codex_state(&path) {
        return;
    }

    if let Ok(canonical) = path.canonicalize() {
        candidates.insert(canonical);
    }
}

fn has_codex_state(path: &Path) -> bool {
    path.join("auth.json").is_file() || path.join("config.toml").is_file()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn state_detection_does_not_open_credentials() {
        let directory = tempfile::tempdir().unwrap();
        assert!(!has_codex_state(directory.path()));
        std::fs::write(directory.path().join("config.toml"), "").unwrap();
        assert!(has_codex_state(directory.path()));
    }

    #[test]
    fn canonicalizes_and_deduplicates_discovered_homes() {
        let user_home = tempfile::tempdir().unwrap();
        let default_home = user_home.path().join(".codex");
        let team_home = user_home.path().join(".codex_team");
        std::fs::create_dir(&default_home).unwrap();
        std::fs::create_dir(&team_home).unwrap();
        std::fs::write(team_home.join("config.toml"), "").unwrap();
        symlink(&default_home, user_home.path().join(".codex_alias")).unwrap();

        let homes = candidate_homes_from(
            Some(user_home.path().join(".codex_alias")),
            Some(user_home.path()),
        );

        assert_eq!(
            homes,
            vec![
                default_home.canonicalize().unwrap(),
                team_home.canonicalize().unwrap()
            ]
        );
    }
}
