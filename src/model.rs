use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const DEFAULT_REFRESH_MINUTES: u64 = 5;
pub const MIN_REFRESH_MINUTES: u64 = 1;
pub const MAX_REFRESH_MINUTES: u64 = 1_440;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TileSettings {
    pub codex_home: String,
    pub label: String,
    pub refresh_minutes: u64,
    pub codex_executable: String,
}

impl Default for TileSettings {
    fn default() -> Self {
        Self {
            codex_home: String::new(),
            label: String::new(),
            refresh_minutes: DEFAULT_REFRESH_MINUTES,
            codex_executable: "codex".to_owned(),
        }
    }
}

impl TileSettings {
    pub fn refresh_interval_minutes(&self) -> u64 {
        self.refresh_minutes
            .clamp(MIN_REFRESH_MINUTES, MAX_REFRESH_MINUTES)
    }

    pub fn effective_label(&self) -> String {
        let custom = self.label.trim();
        if !custom.is_empty() {
            return custom.to_owned();
        }

        let directory = self
            .codex_home
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .trim_start_matches('.');

        if directory.is_empty() {
            "CL".to_owned()
        } else {
            directory.to_owned()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitWindow {
    pub used_percent: u8,
    pub remaining_percent: u8,
    pub duration_minutes: Option<u64>,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResetCredits {
    pub available_count: u64,
    pub earliest_expires_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub windows: Vec<LimitWindow>,
    pub reset_credits: Option<ResetCredits>,
}

impl LimitWindow {
    fn from_wire(window: WireWindow) -> Self {
        let used = window.used_percent.clamp(0, 100) as u8;
        Self {
            used_percent: used,
            remaining_percent: 100 - used,
            duration_minutes: window.window_duration_mins,
            resets_at: window.resets_at,
        }
    }

    pub fn label(&self) -> String {
        duration_label(self.duration_minutes)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NormalizeError {
    #[error("the app-server response did not contain a rate-limit snapshot")]
    MissingSnapshot,
    #[error("the app-server response could not be decoded: {0}")]
    InvalidResponse(String),
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireWindow {
    used_percent: i64,
    window_duration_mins: Option<u64>,
    resets_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireSnapshot {
    limit_id: Option<String>,
    primary: Option<WireWindow>,
    secondary: Option<WireWindow>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireResetCredit {
    status: Option<String>,
    expires_at: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireResetCredits {
    available_count: Option<u64>,
    credits: Option<Vec<WireResetCredit>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireResponse {
    rate_limits: Option<WireSnapshot>,
    rate_limits_by_limit_id: Option<HashMap<String, WireSnapshot>>,
    rate_limit_reset_credits: Option<WireResetCredits>,
}

pub fn normalize_usage_snapshot(value: &Value) -> Result<UsageSnapshot, NormalizeError> {
    let result = value.get("result").unwrap_or(value);
    let wire: WireResponse = serde_json::from_value(result.clone())
        .map_err(|error| NormalizeError::InvalidResponse(error.to_string()))?;

    let snapshot = wire
        .rate_limits_by_limit_id
        .as_ref()
        .and_then(|buckets| buckets.get("codex").cloned())
        .or_else(|| {
            wire.rate_limits_by_limit_id.as_ref().and_then(|buckets| {
                buckets
                    .values()
                    .find(|bucket| bucket.limit_id.as_deref() == Some("codex"))
                    .cloned()
            })
        })
        .or(wire.rate_limits)
        .ok_or(NormalizeError::MissingSnapshot)?;

    let mut windows: Vec<_> = [snapshot.primary, snapshot.secondary]
        .into_iter()
        .flatten()
        .map(LimitWindow::from_wire)
        .collect();

    windows.sort_by_key(|window| window.duration_minutes.unwrap_or(u64::MAX));
    windows.dedup_by_key(|window| window.duration_minutes);

    if windows.len() > 2 {
        let last = windows.pop().expect("length checked");
        windows.truncate(1);
        windows.push(last);
    }

    let reset_credits = wire.rate_limit_reset_credits.and_then(|summary| {
        summary.available_count.map(|available_count| {
            let earliest_expires_at = (available_count > 0)
                .then(|| {
                    summary.credits.as_deref().and_then(|credits| {
                        credits
                            .iter()
                            .filter(|credit| {
                                credit
                                    .status
                                    .as_deref()
                                    .is_none_or(|status| status == "available")
                            })
                            .filter_map(|credit| credit.expires_at)
                            .min()
                    })
                })
                .flatten();

            ResetCredits {
                available_count,
                earliest_expires_at,
            }
        })
    });

    Ok(UsageSnapshot {
        windows,
        reset_credits,
    })
}

#[cfg(test)]
pub fn normalize_rate_limits(value: &Value) -> Result<Vec<LimitWindow>, NormalizeError> {
    normalize_usage_snapshot(value).map(|snapshot| snapshot.windows)
}

pub fn duration_label(duration_minutes: Option<u64>) -> String {
    let Some(minutes) = duration_minutes else {
        return "Limit".to_owned();
    };

    if minutes >= 1_440 {
        if minutes % 1_440 == 0 {
            return format!("{}d", minutes / 1_440);
        }
        return format!("~{}d", (minutes as f64 / 1_440.0).round() as u64);
    }

    if minutes >= 60 {
        if minutes % 60 == 0 {
            return format!("{}h", minutes / 60);
        }
        return format!("~{}h", (minutes as f64 / 60.0).round() as u64);
    }

    format!("{minutes}m")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(contents: &str) -> Value {
        serde_json::from_str(contents).unwrap()
    }

    #[test]
    fn clamps_settings_refresh_interval() {
        let mut settings = TileSettings {
            refresh_minutes: 0,
            ..TileSettings::default()
        };
        assert_eq!(settings.refresh_interval_minutes(), 1);
        settings.refresh_minutes = 9_999;
        assert_eq!(settings.refresh_interval_minutes(), 1_440);
    }

    #[test]
    fn uses_custom_or_directory_name_for_tile_header() {
        let mut settings = TileSettings {
            codex_home: "/home/emil/.codex_contextivo".into(),
            ..TileSettings::default()
        };
        assert_eq!(settings.effective_label(), "codex_contextivo");

        settings.label = "PLUS".into();
        assert_eq!(settings.effective_label(), "PLUS");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration_label(Some(300)), "5h");
        assert_eq!(duration_label(Some(10_080)), "7d");
        assert_eq!(duration_label(Some(43_800)), "~30d");
        assert_eq!(duration_label(Some(45)), "45m");
        assert_eq!(duration_label(None), "Limit");
    }

    #[test]
    fn parses_legacy_windows_and_remaining_percent() {
        let value = fixture(include_str!("../tests/fixtures/legacy-5h-7d.json"));
        let windows = normalize_rate_limits(&value).unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label(), "5h");
        assert_eq!(windows[0].remaining_percent, 75);
        assert_eq!(windows[1].label(), "7d");
        assert_eq!(windows[1].remaining_percent, 58);
    }

    #[test]
    fn prefers_the_codex_bucket() {
        let value = fixture(include_str!("../tests/fixtures/multiple-buckets.json"));
        let windows = normalize_rate_limits(&value).unwrap();
        assert_eq!(windows[0].used_percent, 7);
        assert_eq!(windows[0].label(), "7d");
    }

    #[test]
    fn accepts_a_current_single_weekly_window() {
        let value = fixture(include_str!("../tests/fixtures/single-7d.json"));
        let windows = normalize_rate_limits(&value).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label(), "7d");
        assert_eq!(windows[0].remaining_percent, 93);
        assert_eq!(windows[0].resets_at, Some(1_785_832_053));
    }

    #[test]
    fn accepts_a_single_monthly_window() {
        let value = fixture(include_str!("../tests/fixtures/single-30d.json"));
        let windows = normalize_rate_limits(&value).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label(), "~30d");
        assert_eq!(windows[0].remaining_percent, 83);
    }

    #[test]
    fn clamps_wire_percentages() {
        let value =
            fixture(r#"{"rateLimits":{"primary":{"usedPercent":140,"windowDurationMins":300}}}"#);
        assert_eq!(
            normalize_rate_limits(&value).unwrap()[0].remaining_percent,
            0
        );
    }

    #[test]
    fn allows_a_snapshot_without_windows() {
        let value = fixture(r#"{"rateLimits":{"limitId":"codex"}}"#);
        assert!(normalize_rate_limits(&value).unwrap().is_empty());
    }

    #[test]
    fn ignores_null_windows_and_unknown_future_fields() {
        let value = fixture(
            r#"{
                "futureTopLevel": {"anything": true},
                "rateLimits": {
                    "limitId": "codex",
                    "primary": null,
                    "secondary": {
                        "usedPercent": 30,
                        "windowDurationMins": 720,
                        "futureWindowField": "ignored"
                    },
                    "futureSnapshotField": [1, 2, 3]
                }
            }"#,
        );
        let windows = normalize_rate_limits(&value).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label(), "12h");
        assert_eq!(windows[0].remaining_percent, 70);
    }

    #[test]
    fn parses_authoritative_reset_count_and_earliest_available_expiry() {
        let value = fixture(
            r#"{
                "rateLimits": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 25, "windowDurationMins": 300}
                },
                "rateLimitResetCredits": {
                    "availableCount": 4,
                    "credits": [
                        {"status": "available", "expiresAt": 1784246400},
                        {"status": "redeemed", "expiresAt": 1781000000},
                        {"status": "available", "expiresAt": 1781654400}
                    ]
                }
            }"#,
        );
        let snapshot = normalize_usage_snapshot(&value).unwrap();
        assert_eq!(
            snapshot.reset_credits,
            Some(ResetCredits {
                available_count: 4,
                earliest_expires_at: Some(1_781_654_400),
            })
        );
    }

    #[test]
    fn distinguishes_unknown_reset_details_from_no_reset_summary() {
        let count_only = fixture(
            r#"{
                "rateLimits": {"limitId": "codex"},
                "rateLimitResetCredits": {"availableCount": 2, "credits": null}
            }"#,
        );
        assert_eq!(
            normalize_usage_snapshot(&count_only).unwrap().reset_credits,
            Some(ResetCredits {
                available_count: 2,
                earliest_expires_at: None,
            })
        );

        let empty = fixture(
            r#"{
                "rateLimits": {"limitId": "codex"},
                "rateLimitResetCredits": {
                    "availableCount": 0,
                    "credits": [{"status": "available", "expiresAt": 1784246400}]
                }
            }"#,
        );
        assert_eq!(
            normalize_usage_snapshot(&empty).unwrap().reset_credits,
            Some(ResetCredits {
                available_count: 0,
                earliest_expires_at: None,
            })
        );

        let unavailable =
            fixture(r#"{"rateLimits": {"limitId": "codex"}, "rateLimitResetCredits": null}"#);
        assert_eq!(
            normalize_usage_snapshot(&unavailable)
                .unwrap()
                .reset_credits,
            None
        );
    }
}
