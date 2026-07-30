use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use openaction::get_instance;
use serde::Serialize;
use tokio::sync::{Mutex, oneshot};

use crate::app_server::{AppServerClient, AppServerError};
use crate::model::{TileSettings, UsageSnapshot};
use crate::render::{TileView, render_data_uri};

const POLL_RESOLUTION: Duration = Duration::from_secs(5);
const IMAGE_RECONCILE_INTERVAL: Duration = Duration::from_secs(4);

#[derive(Clone, Debug, Eq)]
pub struct AccountKey {
    pub codex_home: PathBuf,
    pub executable: String,
}

impl PartialEq for AccountKey {
    fn eq(&self, other: &Self) -> bool {
        self.codex_home == other.codex_home && self.executable == other.executable
    }
}

impl Hash for AccountKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.codex_home.hash(state);
        self.executable.hash(state);
    }
}

#[derive(Clone)]
struct Subscription {
    settings: TileSettings,
    key: Option<AccountKey>,
    setup_error: Option<AppServerError>,
}

#[derive(Default)]
struct CacheEntry {
    snapshot: Option<UsageSnapshot>,
    last_success: Option<SystemTime>,
    last_attempt: Option<Instant>,
    error: Option<AppServerError>,
    refreshing: bool,
    waiters: Vec<oneshot::Sender<Result<(), AppServerError>>>,
}

#[derive(Default)]
struct ManagerState {
    subscriptions: HashMap<String, Subscription>,
    caches: HashMap<AccountKey, CacheEntry>,
}

#[derive(Default)]
pub struct AccountManager {
    state: Mutex<ManagerState>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InspectorStatus {
    event: &'static str,
    state: &'static str,
    message: String,
    refreshed_at: Option<u64>,
}

impl AccountManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn start(self: &Arc<Self>) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(POLL_RESOLUTION);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                for key in manager.due_keys().await {
                    let manager = manager.clone();
                    tokio::spawn(async move {
                        let _ = manager.refresh_key(key).await;
                    });
                }
            }
        });

        // OpenDeck currently redraws every key from the profile snapshot whenever
        // the layout is edited. Dynamic setImage values are not reflected in that
        // frontend snapshot, so the manifest icon can overwrite a live tile even
        // though this action received no lifecycle event. Reassert cached images
        // independently of Codex polling; this does not start an app-server. The
        // interval stays above OpenDeck's two-second image persistence debounce so
        // the current design is saved instead of an old manifest fallback.
        let manager = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(IMAGE_RECONCILE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                manager.repaint_visible().await;
            }
        });
    }

    pub async fn subscribe(self: &Arc<Self>, instance_id: String, settings: TileSettings) {
        let key_result = account_key(&settings);
        let (key, setup_error) = match key_result {
            Ok(key) => (Some(key), None),
            Err(error) => (None, Some(error)),
        };

        let should_refresh = {
            let mut state = self.state.lock().await;
            state.subscriptions.insert(
                instance_id.clone(),
                Subscription {
                    settings: settings.clone(),
                    key: key.clone(),
                    setup_error,
                },
            );

            key.as_ref().is_some_and(|key| {
                let cache = state.caches.entry(key.clone()).or_default();
                cache.last_attempt.is_none()
                    || cache
                        .last_attempt
                        .is_some_and(|last| last.elapsed() >= refresh_interval(&settings))
            })
        };

        self.render_instance(&instance_id).await;

        if should_refresh && let Some(key) = key {
            let manager = self.clone();
            tokio::spawn(async move {
                let _ = manager.refresh_key(key).await;
            });
        }
    }

    pub async fn unsubscribe(&self, instance_id: &str) {
        self.state.lock().await.subscriptions.remove(instance_id);
        self.repaint_visible().await;
    }

    pub async fn force_refresh(self: &Arc<Self>, instance_id: &str) -> Result<(), AppServerError> {
        let key = {
            let state = self.state.lock().await;
            let subscription = state.subscriptions.get(instance_id).ok_or_else(|| {
                AppServerError::InvalidHome("the tile is not currently visible".into())
            })?;
            subscription.key.clone().ok_or_else(|| {
                subscription
                    .setup_error
                    .clone()
                    .unwrap_or_else(|| AppServerError::InvalidHome("select a CODEX_HOME".into()))
            })?
        };

        self.refresh_key(key).await
    }

    pub async fn send_status(&self, instance_id: &str) {
        self.render_instance(instance_id).await;
    }

    async fn refresh_key(self: &Arc<Self>, key: AccountKey) -> Result<(), AppServerError> {
        let (receiver, should_start, instance_ids) = {
            let mut state = self.state.lock().await;
            let cache = state.caches.entry(key.clone()).or_default();
            let (sender, receiver) = oneshot::channel();
            cache.waiters.push(sender);

            let should_start = !cache.refreshing;
            if should_start {
                cache.refreshing = true;
                cache.last_attempt = Some(Instant::now());
                cache.error = None;
            }

            let instance_ids = subscribers_for_key(&state, &key);
            (receiver, should_start, instance_ids)
        };

        if should_start {
            self.render_instances(instance_ids).await;
            let manager = self.clone();
            tokio::spawn(async move {
                manager.perform_refresh(key).await;
            });
        }

        receiver.await.unwrap_or_else(|_| {
            Err(AppServerError::ProcessExited(
                "the refresh task ended unexpectedly".into(),
            ))
        })
    }

    async fn perform_refresh(self: Arc<Self>, key: AccountKey) {
        let result = AppServerClient::new(key.executable.clone(), key.codex_home.clone())
            .fetch_limits()
            .await;

        let (instance_ids, waiters, waiter_result) = {
            let mut state = self.state.lock().await;
            let (waiters, waiter_result) = {
                let cache = state.caches.entry(key.clone()).or_default();
                cache.refreshing = false;

                let waiter_result = match result {
                    Ok(snapshot) => {
                        cache.snapshot = Some(snapshot);
                        cache.last_success = Some(SystemTime::now());
                        cache.error = None;
                        Ok(())
                    }
                    Err(error) => {
                        cache.error = Some(error.clone());
                        Err(error)
                    }
                };
                (std::mem::take(&mut cache.waiters), waiter_result)
            };

            (subscribers_for_key(&state, &key), waiters, waiter_result)
        };

        self.render_instances(instance_ids).await;
        for waiter in waiters {
            let _ = waiter.send(waiter_result.clone());
        }
    }

    async fn due_keys(&self) -> Vec<AccountKey> {
        let state = self.state.lock().await;
        let mut intervals: HashMap<AccountKey, Duration> = HashMap::new();

        for subscription in state.subscriptions.values() {
            let Some(key) = subscription.key.clone() else {
                continue;
            };
            let interval = refresh_interval(&subscription.settings);
            intervals
                .entry(key)
                .and_modify(|current| *current = (*current).min(interval))
                .or_insert(interval);
        }

        intervals
            .into_iter()
            .filter_map(|(key, interval)| {
                let cache = state.caches.get(&key);
                let due = cache.is_none_or(|cache| {
                    !cache.refreshing
                        && cache
                            .last_attempt
                            .is_none_or(|last| last.elapsed() >= interval)
                });
                due.then_some(key)
            })
            .collect()
    }

    async fn render_instances(&self, instance_ids: Vec<String>) {
        for instance_id in instance_ids {
            self.render_instance(&instance_id).await;
        }
    }

    async fn repaint_visible(&self) {
        let instance_ids = {
            let state = self.state.lock().await;
            state.subscriptions.keys().cloned().collect::<Vec<_>>()
        };

        for instance_id in instance_ids {
            self.repaint_instance(&instance_id).await;
        }
    }

    async fn repaint_instance(&self, instance_id: &str) {
        let Some((view, _)) = self.view_and_status(instance_id).await else {
            return;
        };
        self.set_tile_image(instance_id, &view).await;
    }

    async fn render_instance(&self, instance_id: &str) {
        let Some((view, status)) = self.view_and_status(instance_id).await else {
            return;
        };

        self.set_tile_image(instance_id, &view).await;

        let Some(instance) = get_instance(instance_id.to_owned()).await else {
            return;
        };

        if let Err(error) = instance.send_to_property_inspector(status).await {
            log::debug!("Property inspector is unavailable for {instance_id}: {error}");
        }
    }

    async fn set_tile_image(&self, instance_id: &str, view: &TileView) {
        let Some(instance) = get_instance(instance_id.to_owned()).await else {
            return;
        };

        match render_data_uri(view) {
            Ok(image) => {
                if let Err(error) = instance.set_image(Some(image), None).await {
                    log::error!("Failed to update tile {instance_id}: {error}");
                }
            }
            Err(error) => log::error!("Failed to render tile {instance_id}: {error}"),
        }
    }

    async fn view_and_status(&self, instance_id: &str) -> Option<(TileView, InspectorStatus)> {
        let state = self.state.lock().await;
        let subscription = state.subscriptions.get(instance_id)?;
        let label = subscription.settings.effective_label();

        let Some(key) = &subscription.key else {
            if subscription.settings.codex_home.trim().is_empty() {
                return Some((
                    TileView::Unconfigured,
                    status("unconfigured", "Select a CODEX_HOME", None),
                ));
            }
            let error = subscription.setup_error.as_ref()?;
            return Some((
                TileView::Error {
                    label,
                    message: error.tile_label().into(),
                },
                status("error", error.to_string(), None),
            ));
        };

        let cache = state.caches.get(key);
        match cache {
            None => Some((
                TileView::Loading { label },
                status("loading", "Waiting for first refresh", None),
            )),
            Some(cache) => {
                let refreshed_at = cache.last_success.and_then(unix_seconds);
                match (&cache.snapshot, &cache.error) {
                    (Some(snapshot), error) => {
                        let stale = error.is_some();
                        let message = if cache.refreshing {
                            "Refreshing".to_owned()
                        } else if let Some(error) = error {
                            format!("Stale: {error}")
                        } else {
                            "Up to date".to_owned()
                        };
                        let state_name = if cache.refreshing {
                            "refreshing"
                        } else if stale {
                            "stale"
                        } else {
                            "ready"
                        };
                        Some((
                            TileView::Limits {
                                label,
                                windows: snapshot.windows.clone(),
                                reset_credits: snapshot.reset_credits.clone(),
                                refreshing: cache.refreshing,
                                stale,
                            },
                            status(state_name, &message, refreshed_at),
                        ))
                    }
                    (None, Some(error)) => Some((
                        TileView::Error {
                            label,
                            message: error.tile_label().into(),
                        },
                        status("error", error.to_string(), refreshed_at),
                    )),
                    (None, None) => Some((
                        TileView::Loading { label },
                        status("loading", "Fetching limits", refreshed_at),
                    )),
                }
            }
        }
    }
}

fn status(
    state: &'static str,
    message: impl Into<String>,
    refreshed_at: Option<u64>,
) -> InspectorStatus {
    InspectorStatus {
        event: "status",
        state,
        message: message.into(),
        refreshed_at,
    }
}

fn subscribers_for_key(state: &ManagerState, key: &AccountKey) -> Vec<String> {
    state
        .subscriptions
        .iter()
        .filter_map(|(instance_id, subscription)| {
            (subscription.key.as_ref() == Some(key)).then_some(instance_id.clone())
        })
        .collect()
}

fn refresh_interval(settings: &TileSettings) -> Duration {
    Duration::from_secs(settings.refresh_interval_minutes() * 60)
}

pub fn account_key(settings: &TileSettings) -> Result<AccountKey, AppServerError> {
    let raw_home = settings.codex_home.trim();
    if raw_home.is_empty() {
        return Err(AppServerError::InvalidHome("select a CODEX_HOME".into()));
    }

    let expanded = expand_tilde(raw_home)?;
    let canonical = expanded
        .canonicalize()
        .map_err(|_| AppServerError::InvalidHome(expanded.to_string_lossy().into_owned()))?;
    if !canonical.is_dir() {
        return Err(AppServerError::InvalidHome(
            canonical.to_string_lossy().into_owned(),
        ));
    }

    let executable = settings.codex_executable.trim();
    if executable.is_empty() {
        return Err(AppServerError::CodexNotFound(
            "the executable setting is empty".into(),
        ));
    }

    Ok(AccountKey {
        codex_home: canonical,
        executable: executable.to_owned(),
    })
}

fn expand_tilde(value: &str) -> Result<PathBuf, AppServerError> {
    if value == "~" || value.starts_with("~/") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| AppServerError::InvalidHome("HOME is not set".into()))?;
        if value == "~" {
            Ok(home)
        } else {
            Ok(home.join(&value[2..]))
        }
    } else {
        Ok(PathBuf::from(value))
    }
}

fn unix_seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn counting_codex(directory: &std::path::Path) -> (PathBuf, PathBuf) {
        let executable = directory.join("counting-codex");
        let counter = directory.join("requests");
        let script = format!(
            r#"#!/bin/sh
IFS= read -r initialize
printf '%s\n' '{{"id":0,"result":{{"userAgent":"fake"}}}}'
IFS= read -r initialized
IFS= read -r request
sleep 0.05
count=0
if [ -f "{counter}" ]; then
  IFS= read -r count < "{counter}"
fi
count=$((count + 1))
printf '%s\n' "$count" > "{counter}"
printf '%s\n' '{{"id":2,"result":{{"rateLimits":{{"limitId":"codex","primary":{{"usedPercent":12,"windowDurationMins":300}}}}}}}}'
"#,
            counter = counter.display()
        );
        std::fs::write(&executable, script).unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        (executable, counter)
    }

    async fn wait_for_count(counter: &std::path::Path, expected: u64) {
        for _ in 0..100 {
            let count = std::fs::read_to_string(counter)
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .unwrap_or(0);
            if count >= expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("counter did not reach {expected}");
    }

    #[test]
    fn canonicalizes_and_keys_account_settings() {
        let directory = tempfile::tempdir().unwrap();
        let settings = TileSettings {
            codex_home: directory.path().to_string_lossy().into_owned(),
            codex_executable: "/usr/bin/codex".into(),
            ..TileSettings::default()
        };
        let key = account_key(&settings).unwrap();
        assert_eq!(key.codex_home, directory.path().canonicalize().unwrap());
        assert_eq!(key.executable, "/usr/bin/codex");
    }

    #[test]
    fn rejects_missing_homes() {
        let settings = TileSettings {
            codex_home: "/definitely/not/a/codex/home".into(),
            ..TileSettings::default()
        };
        assert!(matches!(
            account_key(&settings),
            Err(AppServerError::InvalidHome(_))
        ));
    }

    #[tokio::test]
    async fn shared_tiles_use_one_in_flight_request_and_clicks_are_single_flight() {
        let directory = tempfile::tempdir().unwrap();
        let (executable, counter) = counting_codex(directory.path());
        let settings = TileSettings {
            codex_home: directory.path().to_string_lossy().into_owned(),
            codex_executable: executable.to_string_lossy().into_owned(),
            ..TileSettings::default()
        };
        let manager = AccountManager::new();

        manager.subscribe("tile-a".into(), settings.clone()).await;
        manager.subscribe("tile-b".into(), settings).await;
        wait_for_count(&counter, 1).await;
        assert_eq!(std::fs::read_to_string(&counter).unwrap().trim(), "1");

        // Repainting cached tiles after an OpenDeck layout redraw must not
        // produce another app-server request.
        manager.repaint_visible().await;
        manager.repaint_visible().await;
        assert_eq!(std::fs::read_to_string(&counter).unwrap().trim(), "1");

        let (left, right) = tokio::join!(
            manager.force_refresh("tile-a"),
            manager.force_refresh("tile-b")
        );
        left.unwrap();
        right.unwrap();
        wait_for_count(&counter, 2).await;
        assert_eq!(std::fs::read_to_string(&counter).unwrap().trim(), "2");
    }

    #[tokio::test]
    async fn settings_changes_move_a_tile_between_accounts() {
        let directory = tempfile::tempdir().unwrap();
        let first_home = directory.path().join("first");
        let second_home = directory.path().join("second");
        std::fs::create_dir(&first_home).unwrap();
        std::fs::create_dir(&second_home).unwrap();

        let first_settings = TileSettings {
            codex_home: first_home.to_string_lossy().into_owned(),
            codex_executable: "/definitely/missing/codex".into(),
            ..TileSettings::default()
        };
        let second_settings = TileSettings {
            codex_home: second_home.to_string_lossy().into_owned(),
            ..first_settings.clone()
        };
        let first_key = account_key(&first_settings).unwrap();
        let second_key = account_key(&second_settings).unwrap();
        let manager = AccountManager::new();

        manager
            .subscribe("moving-tile".into(), first_settings)
            .await;
        manager
            .subscribe("moving-tile".into(), second_settings)
            .await;

        let state = manager.state.lock().await;
        assert!(subscribers_for_key(&state, &first_key).is_empty());
        assert_eq!(
            subscribers_for_key(&state, &second_key),
            vec!["moving-tile"]
        );
    }
}
