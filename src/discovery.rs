use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use tokio::task::JoinSet;

use crate::app_server::{AppServerClient, canonical_home};

const MAX_CONCURRENT_DISCOVERIES: usize = 4;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountIdentity {
    codex_home: String,
    email: Option<String>,
    plan_type: Option<String>,
    signed_in: bool,
}

impl AccountIdentity {
    async fn discover(executable: String, home: PathBuf) -> Self {
        let codex_home = home.display().to_string();
        let result = AppServerClient::new(executable, home)
            .read_account()
            .await
            .ok();
        let account = result
            .as_ref()
            .and_then(|value| value.get("account"))
            .filter(|account| !account.is_null());
        let field = |key| {
            account
                .and_then(|value| value.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        Self {
            codex_home,
            email: field("email"),
            plan_type: field("planType"),
            signed_in: account.is_some(),
        }
    }
}

pub async fn discover_accounts(executable: &str) -> Vec<AccountIdentity> {
    let inherited = std::env::var_os("CODEX_HOME").map(PathBuf::from);
    let user_home = std::env::var_os("HOME").map(PathBuf::from);
    discover_homes(executable, candidate_homes(inherited, user_home.as_deref())).await
}

async fn discover_homes(executable: &str, candidates: Vec<PathBuf>) -> Vec<AccountIdentity> {
    let mut tasks = JoinSet::new();
    let mut accounts = Vec::with_capacity(candidates.len());
    let mut candidates = candidates.into_iter();
    loop {
        while tasks.len() < MAX_CONCURRENT_DISCOVERIES
            && let Some(home) = candidates.next()
        {
            tasks.spawn(AccountIdentity::discover(executable.to_owned(), home));
        }
        match tasks.join_next().await {
            Some(Ok(account)) => accounts.push(account),
            Some(Err(error)) => log::warn!("Account discovery task failed: {error}"),
            None => break,
        }
    }
    accounts.sort_by(|left, right| left.codex_home.cmp(&right.codex_home));
    accounts
}

fn candidate_homes(inherited: Option<PathBuf>, user_home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths: Vec<_> = inherited.into_iter().collect();
    if let Some(home) = user_home {
        paths.push(home.join(".codex"));
        if let Ok(entries) = home.read_dir() {
            paths.extend(entries.flatten().filter_map(|entry| {
                let path = entry.path();
                let is_codex = entry.file_name().to_string_lossy().starts_with(".codex");
                (is_codex
                    && (path.join("auth.json").is_file() || path.join("config.toml").is_file()))
                .then_some(path)
            }));
        }
    }
    paths
        .into_iter()
        .filter_map(|path| canonical_home(&path).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::time::Duration;

    use super::*;
    use crate::app_server::FAKE_CODEX;

    #[tokio::test]
    async fn discovers_every_home_in_order_with_bounded_parallelism() {
        let directory = tempfile::tempdir().unwrap();
        let homes: Vec<_> = (0..9)
            .rev()
            .map(|i| {
                let home = directory.path().join(format!("home-{i}"));
                fs::create_dir(&home).unwrap();
                symlink(directory.path(), home.join("discovery")).unwrap();
                home
            })
            .collect();
        let events = directory.path().join("events");
        fs::write(&events, "").unwrap();
        let hold = directory.path().join("hold");
        fs::write(&hold, "").unwrap();

        let discovery = tokio::spawn(discover_homes(FAKE_CODEX, homes.clone()));
        tokio::time::timeout(Duration::from_secs(5), async {
            while fs::read_to_string(&events).unwrap().lines().count() < MAX_CONCURRENT_DISCOVERIES
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the first group of accounts should be queried concurrently");
        fs::remove_file(hold).unwrap();
        let accounts = discovery.await.unwrap();
        let expected = homes
            .iter()
            .rev()
            .map(|home| home.display().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            accounts
                .iter()
                .map(|account| account.codex_home.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(accounts.iter().all(|account| account.signed_in
            && account.email.as_deref() == Some("test@example.com")
            && account.plan_type.as_deref() == Some("plus")));

        let mut active: usize = 0;
        for event in fs::read_to_string(events).unwrap().lines() {
            match event {
                "start" => active += 1,
                "end" => active -= 1,
                other => panic!("unexpected event {other}"),
            }
            assert!(active <= MAX_CONCURRENT_DISCOVERIES);
        }
        assert_eq!(active, 0);
    }

    #[tokio::test]
    async fn unavailable_identity_still_offers_the_home() {
        let directory = tempfile::tempdir().unwrap();
        let accounts =
            discover_homes("/definitely/missing/codex", vec![directory.path().into()]).await;
        assert_eq!(
            serde_json::to_value(accounts).unwrap(),
            serde_json::json!([{
                "codexHome": directory.path(),
                "email": null,
                "planType": null,
                "signedIn": false
            }])
        );
    }

    #[test]
    fn includes_explicit_homes_and_only_stateful_neighbors_once() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path();
        for name in [
            ".codex",
            ".codex_team",
            ".codex_personal",
            ".codex_empty",
            "unrelated",
            "inherited",
        ] {
            fs::create_dir(home.join(name)).unwrap();
        }
        fs::write(home.join(".codex_team/config.toml"), "").unwrap();
        fs::write(home.join(".codex_personal/auth.json"), "unparseable").unwrap();
        fs::write(home.join("unrelated/auth.json"), "").unwrap();
        symlink(home.join(".codex_team"), home.join(".codex_alias")).unwrap();
        let homes = candidate_homes(Some(home.join("inherited")), Some(home));
        let expected = [".codex", ".codex_personal", ".codex_team", "inherited"]
            .map(|name| home.join(name).canonicalize().unwrap());
        assert_eq!(homes, expected);
    }
}
