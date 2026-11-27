use std::sync::Arc;

use openaction::{Action, Instance, OpenActionResult, async_trait, get_instance};
use serde_json::{Value, json};

use crate::account_manager::AccountManager;
use crate::discovery::discover_accounts;
use crate::settings::{DEFAULT_EXECUTABLE, TileSettings};

pub struct CodexLimitsAction {
    manager: Arc<AccountManager>,
}

impl CodexLimitsAction {
    pub fn new(manager: Arc<AccountManager>) -> Self {
        Self { manager }
    }

    async fn spawn_refresh_with_feedback(&self, instance_id: String) {
        let result = self.manager.start_manual_refresh(instance_id.clone()).await;
        tokio::spawn(async move {
            let Some(instance) = get_instance(instance_id.clone()).await else {
                return;
            };

            let Some(result) = result.await else {
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

    fn spawn_account_discovery(instance_id: String, executable: &str) {
        let executable = if executable.trim().is_empty() {
            DEFAULT_EXECUTABLE.to_owned()
        } else {
            executable.trim().to_owned()
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
            .subscribe(instance.instance_id.clone(), settings)
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
        self.spawn_refresh_with_feedback(instance.instance_id.clone())
            .await;
        Ok(())
    }

    async fn did_receive_settings(
        &self,
        instance: &Instance,
        settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        self.manager
            .subscribe(instance.instance_id.clone(), settings)
            .await;
        Ok(())
    }

    async fn property_inspector_did_appear(
        &self,
        instance: &Instance,
        _settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        self.manager.publish_tile(&instance.instance_id).await;
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
                let executable = payload
                    .get("codexExecutable")
                    .and_then(Value::as_str)
                    .unwrap_or(&settings.codex_executable);
                Self::spawn_account_discovery(instance.instance_id.clone(), executable);
            }
            Some("refreshNow") => {
                self.spawn_refresh_with_feedback(instance.instance_id.clone())
                    .await
            }
            Some(other) => log::debug!("Ignoring unknown property inspector event: {other}"),
            None => {}
        }
        Ok(())
    }
}
