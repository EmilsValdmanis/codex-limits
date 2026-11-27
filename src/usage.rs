use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitWindow {
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
            remaining_percent: 100 - used,
            duration_minutes: window.window_duration_mins,
            resets_at: window.resets_at,
        }
    }

    pub fn label(&self) -> String {
        let Some(minutes) = self.duration_minutes else {
            return "Limit".to_owned();
        };
        if minutes >= 1_440 {
            let days = minutes / 1_440 + u64::from(minutes % 1_440 >= 720);
            return format!("{days}d");
        }
        if minutes >= 60 {
            let prefix = if minutes.is_multiple_of(60) { "" } else { "~" };
            return format!("{prefix}{}h", (minutes + 30) / 60);
        }
        format!("{minutes}m")
    }

    pub fn time_remaining_percent(&self, now: i64) -> Option<u8> {
        let minutes = self.duration_minutes.filter(|&minutes| minutes > 0)?;
        let seconds_left = self.resets_at?.saturating_sub(now);
        let share = seconds_left as f64 / (minutes as f64 * 60.0);
        Some((share.clamp(0.0, 1.0) * 100.0).round() as u8)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NormalizeError {
    #[error("the app-server response did not contain a rate-limit snapshot")]
    MissingSnapshot,
    #[error("the app-server response could not be decoded: {0}")]
    InvalidResponse(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireWindow {
    used_percent: i64,
    window_duration_mins: Option<u64>,
    resets_at: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireSnapshot {
    limit_id: Option<String>,
    primary: Option<WireWindow>,
    secondary: Option<WireWindow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireResetCredit {
    status: Option<String>,
    expires_at: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireResetCredits {
    available_count: Option<u64>,
    credits: Option<Vec<WireResetCredit>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireResponse {
    rate_limits: Option<WireSnapshot>,
    rate_limits_by_limit_id: Option<HashMap<String, WireSnapshot>>,
    rate_limit_reset_credits: Option<WireResetCredits>,
}

pub fn normalize_usage_snapshot(value: &Value) -> Result<UsageSnapshot, NormalizeError> {
    let wire = WireResponse::deserialize(value)
        .map_err(|error| NormalizeError::InvalidResponse(error.to_string()))?;

    let snapshot = wire
        .rate_limits_by_limit_id
        .and_then(|mut buckets| {
            buckets.remove("codex").or_else(|| {
                buckets
                    .into_values()
                    .find(|bucket| bucket.limit_id.as_deref() == Some("codex"))
            })
        })
        .or(wire.rate_limits)
        .ok_or(NormalizeError::MissingSnapshot)?;

    let mut windows: Vec<_> = [snapshot.primary, snapshot.secondary]
        .into_iter()
        .flatten()
        .map(LimitWindow::from_wire)
        .collect();

    windows.sort_by_key(|window| (window.duration_minutes.is_none(), window.duration_minutes));
    // Missing durations do not establish that two windows are duplicates.
    windows.dedup_by(|left, right| {
        left.duration_minutes.is_some() && left.duration_minutes == right.duration_minutes
    });

    Ok(UsageSnapshot {
        windows,
        reset_credits: wire
            .rate_limit_reset_credits
            .and_then(WireResetCredits::into_summary),
    })
}

impl WireResetCredits {
    fn into_summary(self) -> Option<ResetCredits> {
        let available_count = self.available_count?;
        let earliest_expires_at = if available_count == 0 {
            None
        } else {
            self.credits
                .into_iter()
                .flatten()
                .filter(|credit| {
                    credit
                        .status
                        .as_deref()
                        .is_none_or(|status| status == "available")
                })
                .filter_map(|credit| credit.expires_at)
                .min()
        };
        Some(ResetCredits {
            available_count,
            earliest_expires_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(contents: &str) -> Value {
        serde_json::from_str(contents).unwrap()
    }

    #[test]
    fn formats_durations() {
        for (duration_minutes, expected) in [
            (Some(300), "5h"),
            (Some(90), "~2h"),
            (Some(10_080), "7d"),
            (Some(43_800), "30d"),
            (Some(45), "45m"),
            (None, "Limit"),
        ] {
            let window = LimitWindow {
                remaining_percent: 0,
                duration_minutes,
                resets_at: None,
            };
            assert_eq!(window.label(), expected);
        }
    }

    #[test]
    fn parses_legacy_windows_and_remaining_percent() {
        let response = fixture(include_str!("../tests/fixtures/legacy-5h-7d.json"));
        let windows = normalize_usage_snapshot(&response["result"])
            .unwrap()
            .windows;
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label(), "5h");
        assert_eq!(windows[0].remaining_percent, 75);
        assert_eq!(windows[1].label(), "7d");
        assert_eq!(windows[1].remaining_percent, 58);
    }

    #[test]
    fn prefers_the_codex_bucket() {
        let response = fixture(include_str!("../tests/fixtures/multiple-buckets.json"));
        let windows = normalize_usage_snapshot(&response["result"])
            .unwrap()
            .windows;
        assert_eq!(windows[0].remaining_percent, 93);
        assert_eq!(windows[0].label(), "7d");
    }

    #[test]
    fn preserves_windows_with_unknown_durations() {
        let value = fixture(
            r#"{"rateLimits": {
                "primary": {"usedPercent": 10},
                "secondary": {"usedPercent": 80}
            }}"#,
        );
        let windows = normalize_usage_snapshot(&value).unwrap().windows;
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].remaining_percent, 90);
        assert_eq!(windows[1].remaining_percent, 20);
    }

    #[test]
    fn deduplicates_windows_with_matching_known_durations() {
        let value = fixture(
            r#"{"rateLimits": {
                "primary": {"usedPercent": 10, "windowDurationMins": 300},
                "secondary": {"usedPercent": 10, "windowDurationMins": 300}
            }}"#,
        );
        assert_eq!(normalize_usage_snapshot(&value).unwrap().windows.len(), 1);
    }

    #[test]
    fn finds_codex_by_limit_id_when_the_bucket_key_differs() {
        let value = fixture(
            r#"{
                "rateLimits": {"primary": {"usedPercent": 90}},
                "rateLimitsByLimitId": {
                    "other-key": {"limitId": "codex", "primary": {"usedPercent": 10}}
                }
            }"#,
        );
        assert_eq!(
            normalize_usage_snapshot(&value).unwrap().windows[0].remaining_percent,
            90
        );
    }

    #[test]
    fn accepts_a_current_single_weekly_window() {
        let response = fixture(include_str!("../tests/fixtures/single-7d.json"));
        let windows = normalize_usage_snapshot(&response["result"])
            .unwrap()
            .windows;
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label(), "7d");
        assert_eq!(windows[0].remaining_percent, 93);
        assert_eq!(windows[0].resets_at, Some(1_785_832_053));
    }

    #[test]
    fn accepts_a_single_monthly_window() {
        let response = fixture(include_str!("../tests/fixtures/single-30d.json"));
        let windows = normalize_usage_snapshot(&response["result"])
            .unwrap()
            .windows;
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label(), "30d");
        assert_eq!(windows[0].remaining_percent, 83);
    }

    #[test]
    fn clamps_wire_percentages() {
        let value =
            fixture(r#"{"rateLimits":{"primary":{"usedPercent":140,"windowDurationMins":300}}}"#);
        assert_eq!(
            normalize_usage_snapshot(&value).unwrap().windows[0].remaining_percent,
            0
        );
    }

    #[test]
    fn allows_a_snapshot_without_windows() {
        let value = fixture(r#"{"rateLimits":{"limitId":"codex"}}"#);
        assert!(normalize_usage_snapshot(&value).unwrap().windows.is_empty());
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
        let windows = normalize_usage_snapshot(&value).unwrap().windows;
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
