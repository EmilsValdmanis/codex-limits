use std::sync::Arc;

use openaction::{Action, Instance, OpenActionResult, async_trait, get_instance};
use serde_json::{Value, json};

use crate::account_manager::AccountManager;
use crate::discovery::discover_accounts;
use crate::model::TileSettings;

pub struct CodexLimitsAction {
    manager: Arc<AccountManager>,
}

impl CodexLimitsAction {
    pub fn new(manager: Arc<AccountManager>) -> Self {
        Self { manager }
    }

    fn spawn_refresh_with_feedback(&self, instance_id: String) {
        let manager = self.manager.clone();
        tokio::spawn(async move {
            let result = manager.force_refresh(&instance_id).await;
            let Some(instance) = get_instance(instance_id.clone()).await else {
                return;
            };

            match result {
                Ok(()) => {
                    if let Err(error) = instance.show_ok().await {
                        log::debug!("Could not show refresh success for {instance_id}: {error}");
                    }
                }
                Err(error) => {
                    log::warn!("Manual refresh failed for {instance_id}: {error}");
                    if let Err(feedback_error) = instance.show_alert().await {
                        log::debug!(
                            "Could not show refresh failure for {instance_id}: {feedback_error}"
                        );
                    }
                }
            }
        });
    }

    fn spawn_account_discovery(&self, instance_id: String, settings: &TileSettings) {
        let executable = if settings.codex_executable.trim().is_empty() {
            "codex".to_owned()
        } else {
            settings.codex_executable.trim().to_owned()
        };

        tokio::spawn(async move {
            let accounts = discover_accounts(&executable).await;
            let Some(instance) = get_instance(instance_id.clone()).await else {
                return;
            };
            if let Err(error) = instance
                .send_to_property_inspector(json!({
                    "event": "accountsDiscovered",
                    "accounts": accounts
                }))
                .await
            {
                log::debug!("Could not send discovered accounts to {instance_id}: {error}");
            }
        });
    }
}

#[async_trait]
impl Action for CodexLimitsAction {
    const UUID: &'static str = "com.emilsvaldmanis.codexlimits.account";
    type Settings = TileSettings;

    async fn will_appear(
        &self,
        instance: &Instance,
        settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        self.manager
            .subscribe(instance.instance_id.clone(), settings.clone())
            .await;
        Ok(())
    }

    async fn will_disappear(
        &self,
        instance: &Instance,
        _settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        self.manager.unsubscribe(&instance.instance_id).await;
        Ok(())
    }

    async fn key_up(
        &self,
        instance: &Instance,
        _settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        self.spawn_refresh_with_feedback(instance.instance_id.clone());
        Ok(())
    }

    async fn did_receive_settings(
        &self,
        instance: &Instance,
        settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        self.manager
            .subscribe(instance.instance_id.clone(), settings.clone())
            .await;
        Ok(())
    }

    async fn property_inspector_did_appear(
        &self,
        instance: &Instance,
        _settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        self.manager.send_status(&instance.instance_id).await;
        Ok(())
    }

    async fn send_to_plugin(
        &self,
        instance: &Instance,
        settings: &Self::Settings,
        payload: &Value,
    ) -> OpenActionResult<()> {
        match payload.get("event").and_then(Value::as_str) {
            Some("discoverAccounts") => {
                self.spawn_account_discovery(instance.instance_id.clone(), settings);
                Ok(())
            }
            Some("refreshNow") => {
                self.spawn_refresh_with_feedback(instance.instance_id.clone());
                Ok(())
            }
            Some(other) => {
                log::debug!("Ignoring unknown property inspector event: {other}");
                Ok(())
            }
            None => Ok(()),
        }
    }
}
