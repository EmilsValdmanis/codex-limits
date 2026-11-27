use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_EXECUTABLE: &str = "codex";
const DEFAULT_REFRESH_MINUTES: u64 = 5;
const MIN_REFRESH_MINUTES: u64 = 1;
const MAX_REFRESH_MINUTES: u64 = 1_440;

#[derive(Debug, Deserialize, Serialize)]
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
            codex_executable: DEFAULT_EXECUTABLE.to_owned(),
        }
    }
}

impl TileSettings {
    pub fn refresh_interval(&self) -> Duration {
        Duration::from_secs(
            self.refresh_minutes
                .clamp(MIN_REFRESH_MINUTES, MAX_REFRESH_MINUTES)
                * 60,
        )
    }

    pub fn effective_label(&self) -> String {
        let custom = self.label.trim();
        if !custom.is_empty() {
            return custom.to_owned();
        }

        let directory = self
            .codex_home
            .trim()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_settings_refresh_interval() {
        let mut settings = TileSettings {
            refresh_minutes: 0,
            ..TileSettings::default()
        };
        assert_eq!(settings.refresh_interval(), Duration::from_secs(60));
        settings.refresh_minutes = 9_999;
        assert_eq!(settings.refresh_interval(), Duration::from_secs(24 * 3600));
    }

    #[test]
    fn uses_custom_or_directory_name_for_tile_header() {
        let mut settings = TileSettings {
            codex_home: "/home/emil/.codex_contextivo".into(),
            ..TileSettings::default()
        };
        assert_eq!(settings.effective_label(), "codex_contextivo");

        settings.codex_home = "  /home/emil/.codex_contextivo/  ".into();
        assert_eq!(settings.effective_label(), "codex_contextivo");

        settings.label = "PLUS".into();
        assert_eq!(settings.effective_label(), "PLUS");
    }
}
