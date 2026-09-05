use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tokio::task::JoinSet;

use crate::app_server::{AccountIdentity, AppServerClient};

const MAX_CONCURRENT_DISCOVERIES: usize = 4;

pub async fn discover_accounts(executable: &str) -> Vec<AccountIdentity> {
    discover_homes(executable, candidate_homes()).await
}

async fn discover_homes(executable: &str, candidates: Vec<PathBuf>) -> Vec<AccountIdentity> {
    let mut tasks = JoinSet::new();
    let mut accounts = Vec::with_capacity(candidates.len());

    for home in candidates {
        // Each task starts a process. Keep discovery responsive without launching
        // an app-server for every home simultaneously.
        if tasks.len() == MAX_CONCURRENT_DISCOVERIES
            && let Some(Ok(account)) = tasks.join_next().await
        {
            accounts.push(account);
        }
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
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    #[tokio::test]
    async fn limits_concurrent_processes_and_returns_every_home_in_order() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
printf 'start\n' >> "$CODEX_HOME/../events"
IFS= read -r initialize
printf '%s\n' '{"id":0,"result":{}}'
IFS= read -r initialized
IFS= read -r request
sleep 0.05
printf 'end\n' >> "$CODEX_HOME/../events"
printf '%s\n' '{"id":3,"result":{"account":{"planType":"plus"}}}'
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let homes: Vec<_> = (0..9)
            .rev()
            .map(|index| {
                let home = directory.path().join(format!("home-{index}"));
                std::fs::create_dir(&home).unwrap();
                home
            })
            .collect();

        let accounts = discover_homes(executable.to_str().unwrap(), homes.clone()).await;
        assert_eq!(accounts.len(), homes.len());
        assert!(
            accounts
                .iter()
                .all(|account| account.plan_type.as_deref() == Some("plus"))
        );
        let mut expected: Vec<_> = homes
            .iter()
            .map(|home| home.to_string_lossy().into_owned())
            .collect();
        expected.sort();
        assert_eq!(
            accounts
                .iter()
                .map(|account| account.codex_home.clone())
                .collect::<Vec<_>>(),
            expected
        );

        let events = std::fs::read_to_string(directory.path().join("events")).unwrap();
        let mut active = 0;
        let mut peak = 0;
        for event in events.lines() {
            match event {
                "start" => active += 1,
                "end" => active -= 1,
                _ => panic!("unexpected event: {event}"),
            }
            peak = peak.max(active);
            assert!((0..=MAX_CONCURRENT_DISCOVERIES as i32).contains(&active));
        }
        assert_eq!(active, 0);
        assert!(peak > 1, "discovery should still run in parallel");

        let fallback = discover_homes("/definitely/missing/codex", homes).await;
        assert_eq!(fallback.len(), expected.len());
        assert!(
            fallback
                .iter()
                .all(|account| !account.signed_in && account.email.is_none())
        );
    }

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
