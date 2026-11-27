mod presentation;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use openaction::get_instance;
use tokio::sync::{Mutex, broadcast};

use crate::app_server::{AppServerClient, AppServerError};
use crate::settings::TileSettings;
use crate::usage::UsageSnapshot;
use presentation::TilePresentation;

const POLL_RESOLUTION: Duration = Duration::from_secs(5);
// OpenDeck can restore the profile icon after a layout edit without notifying the
// action. Reassert images above its two-second persistence debounce.
const IMAGE_RECONCILE_INTERVAL: Duration = Duration::from_secs(4);

type RefreshResult = Result<(), AppServerError>;

struct Subscription {
    label: String,
    interval: Duration,
    account: Result<AppServerClient, AppServerError>,
}

#[derive(Default)]
enum Refresh {
    #[default]
    Idle,
    Running(broadcast::Receiver<RefreshResult>),
    Failed(AppServerError),
}

#[derive(Default)]
struct CachedAccount {
    reading: Option<(UsageSnapshot, SystemTime)>,
    last_attempt: Option<Instant>,
    refresh: Refresh,
}

impl CachedAccount {
    fn active_request(&self) -> Option<&broadcast::Receiver<RefreshResult>> {
        match &self.refresh {
            Refresh::Running(result) if !result.is_closed() => Some(result),
            _ => None,
        }
    }

    fn is_due(&self, interval: Duration, now: Instant) -> bool {
        self.active_request().is_none()
            && self
                .last_attempt
                .is_none_or(|last| now.duration_since(last) >= interval)
    }
}

#[derive(Default)]
struct ManagerState {
    tiles: HashMap<String, Subscription>,
    accounts: HashMap<AppServerClient, CachedAccount>,
}

impl ManagerState {
    fn due_account<'a>(&self, tile: &'a Subscription, now: Instant) -> Option<&'a AppServerClient> {
        tile.account.as_ref().ok().filter(|account| {
            self.accounts
                .get(*account)
                .is_none_or(|cache| cache.is_due(tile.interval, now))
        })
    }

    fn due_accounts(&self, now: Instant) -> HashSet<AppServerClient> {
        self.tiles
            .values()
            .filter_map(|tile| self.due_account(tile, now).cloned())
            .collect()
    }

    fn subscribers(&self, account: &AppServerClient) -> Vec<String> {
        self.tiles
            .iter()
            .filter(|(_, tile)| tile.account.as_ref().ok() == Some(account))
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn presentation(&self, id: &str) -> Option<TilePresentation> {
        let tile = self.tiles.get(id)?;
        Some(match &tile.account {
            Ok(account) => TilePresentation::cached(&tile.label, &self.accounts[account]),
            Err(error) => TilePresentation::error(&tile.label, error),
        })
    }
}

#[derive(Default)]
pub struct AccountManager {
    state: Mutex<ManagerState>,
    publication: Mutex<()>,
}

impl AccountManager {
    pub fn start() -> Arc<Self> {
        let manager = Arc::new(Self::default());
        let polling = manager.clone();
        tokio::spawn(async move {
            let mut interval = periodic(POLL_RESOLUTION);
            loop {
                interval.tick().await;
                polling.poll_due().await;
            }
        });
        let repainting = manager.clone();
        tokio::spawn(async move {
            let mut interval = periodic(IMAGE_RECONCILE_INTERVAL);
            loop {
                interval.tick().await;
                repainting.repaint_visible().await;
            }
        });
        manager
    }

    pub async fn subscribe(self: &Arc<Self>, id: String, settings: &TileSettings) {
        let subscription = Subscription {
            label: settings.effective_label(),
            interval: settings.refresh_interval(),
            account: AppServerClient::from_settings(settings),
        };
        let mut state = self.state.lock().await;
        let due = state.due_account(&subscription, Instant::now());
        let started = due.is_some();
        if let Some(account) = due {
            self.refresh(account, &mut state);
        }
        state.tiles.insert(id.clone(), subscription);
        drop(state);
        if !started {
            self.publish_tile(&id).await;
        }
    }

    pub async fn unsubscribe(&self, id: &str) {
        self.state.lock().await.tiles.remove(id);
        self.repaint_visible().await;
    }

    pub async fn start_manual_refresh(
        self: &Arc<Self>,
        id: String,
    ) -> impl Future<Output = Option<RefreshResult>> + use<> {
        let mut state = self.state.lock().await;
        let account = state.tiles.get(&id).map(|tile| tile.account.clone());
        let mut pending = account
            .as_ref()
            .and_then(|target| target.as_ref().ok())
            .map(|account| self.refresh(account, &mut state));
        let manager = self.clone();

        async move {
            let account = account?;
            let outcome = match pending.as_mut() {
                Some(result) => result.recv().await.unwrap_or_else(|_| {
                    Err(AppServerError::ProcessExited(
                        "the refresh task ended unexpectedly".into(),
                    ))
                }),
                None => account.as_ref().map(|_| ()).map_err(Clone::clone),
            };
            let state = manager.state.lock().await;
            (state.tiles.get(&id)?.account == account).then_some(outcome)
        }
    }

    async fn poll_due(self: &Arc<Self>) {
        let mut state = self.state.lock().await;
        for account in state.due_accounts(Instant::now()) {
            self.refresh(&account, &mut state);
        }
    }

    fn refresh(
        self: &Arc<Self>,
        account: &AppServerClient,
        state: &mut ManagerState,
    ) -> broadcast::Receiver<RefreshResult> {
        let cache = state.accounts.entry(account.clone()).or_default();
        if let Some(result) = cache.active_request() {
            return result.resubscribe();
        }
        let (sender, receiver) = broadcast::channel(1);
        cache.refresh = Refresh::Running(receiver.resubscribe());
        cache.last_attempt = Some(Instant::now());

        let manager = self.clone();
        let account = account.clone();
        tokio::spawn(async move {
            manager.publish_account(&account).await;
            let result = account.fetch_limits().await;
            let outcome = {
                let mut state = manager.state.lock().await;
                let cache = state.accounts.get_mut(&account).expect("active account");
                match result {
                    Ok(snapshot) => {
                        cache.reading = Some((snapshot, SystemTime::now()));
                        cache.refresh = Refresh::Idle;
                        Ok(())
                    }
                    Err(error) => {
                        cache.refresh = Refresh::Failed(error.clone());
                        Err(error)
                    }
                }
            };
            manager.publish_account(&account).await;
            let _ = sender.send(outcome);
        });
        receiver
    }

    async fn publish_account(&self, account: &AppServerClient) {
        let ids = self.state.lock().await.subscribers(account);
        for id in ids {
            self.publish_tile(&id).await;
        }
    }

    pub async fn publish_tile(&self, id: &str) {
        self.publish(id, true).await;
    }

    async fn repaint_visible(&self) {
        let ids: Vec<_> = self.state.lock().await.tiles.keys().cloned().collect();
        for id in ids {
            self.publish(&id, false).await;
        }
    }

    async fn publish(&self, id: &str, include_status: bool) {
        let _publishing = self.publication.lock().await;
        let Some(instance) = get_instance(id.to_owned()).await else {
            return;
        };
        let Some(tile) = self.state.lock().await.presentation(id) else {
            return;
        };
        tile.send(&instance, include_status).await;
    }
}

fn periodic(period: Duration) -> tokio::time::Interval {
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval
}
