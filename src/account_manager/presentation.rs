use std::time::UNIX_EPOCH;

use openaction::Instance;
use serde::Serialize;

use super::{CachedAccount, Refresh};
use crate::app_server::AppServerError;
use crate::render::{Freshness, TileView, render_data_uri};

pub(super) struct TilePresentation {
    pub(super) view: TileView,
    pub(super) status: InspectorStatus,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InspectorStatus {
    event: &'static str,
    state: &'static str,
    message: String,
    refreshed_at: Option<u64>,
}

impl TilePresentation {
    fn new(
        view: TileView,
        state: &'static str,
        message: impl Into<String>,
        refreshed_at: Option<u64>,
    ) -> Self {
        Self {
            view,
            status: InspectorStatus {
                event: "status",
                state,
                message: message.into(),
                refreshed_at,
            },
        }
    }

    pub(super) fn error(label: &str, error: &AppServerError) -> Self {
        let message = match error {
            AppServerError::HomeNotSelected => {
                return Self::new(
                    TileView::Unconfigured,
                    "unconfigured",
                    "Select a CODEX_HOME",
                    None,
                );
            }
            AppServerError::InvalidHome(_) => "Bad home",
            AppServerError::CodexNotFound(_) => "Codex missing",
            AppServerError::NotAuthenticated => "Sign in",
            AppServerError::MethodUnavailable => "Update Codex",
            AppServerError::Timeout | AppServerError::Upstream(_) => "Offline",
            AppServerError::ProcessExited(_)
            | AppServerError::Protocol(_)
            | AppServerError::EmptyRateLimits => "Unavailable",
        };
        Self::new(
            TileView::Error {
                label: label.to_owned(),
                message: message.into(),
            },
            "error",
            error.to_string(),
            None,
        )
    }

    pub(super) fn cached(label: &str, account: &CachedAccount) -> Self {
        let Some((snapshot, recorded)) = &account.reading else {
            return match &account.refresh {
                Refresh::Failed(error) => Self::error(label, error),
                _ => Self::new(
                    TileView::Loading {
                        label: label.to_owned(),
                    },
                    "loading",
                    "Fetching limits",
                    None,
                ),
            };
        };
        let (freshness, state, message) = match &account.refresh {
            Refresh::Idle => (Freshness::Ready, "ready", "Up to date".into()),
            Refresh::Running(_) => (Freshness::Refreshing, "refreshing", "Refreshing".into()),
            Refresh::Failed(error) => (Freshness::Stale, "stale", format!("Stale: {error}")),
        };
        Self::new(
            TileView::Limits {
                label: label.to_owned(),
                snapshot: snapshot.clone(),
                freshness,
            },
            state,
            message,
            recorded
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|age| age.as_secs()),
        )
    }

    pub(super) async fn send(&self, instance: &Instance, include_status: bool) {
        let id = &instance.instance_id;
        match render_data_uri(&self.view) {
            Ok(image) => {
                if let Err(error) = instance.set_image(Some(image), None).await {
                    log::error!("Failed to update tile {id}: {error}");
                }
            }
            Err(error) => log::error!("Failed to render tile {id}: {error}"),
        }
        if include_status
            && let Err(error) = instance.send_to_property_inspector(&self.status).await
        {
            log::debug!("Property inspector is unavailable for {id}: {error}");
        }
    }
}
