//! Persistent user preferences stored in `~/.sumaru`.
//!
//! The format follows AFNI/SUMA's self-documenting `~/.sumarc` convention:
//! stable `SUMARU_...` keys, an `***ENVIRONMENT` section, and comments that
//! explain every setting, allowed value, and default. It remains tolerant of
//! unknown keys so newer Sumaru versions do not make older builds reject the
//! file.

use std::ffi::OsString;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const PREFERENCES_FILE_NAME: &str = ".sumaru";

/// How a threshold is transferred when the active overlay changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayThresholdSync {
    /// Keep the current numeric slider value.  This is predictable even when
    /// the destination column has no statistical metadata.
    #[default]
    CurrentValue,
    /// Preserve the current p-value when both source and destination columns
    /// have compatible stat metadata; otherwise keep the numeric value.
    MatchPValue,
    /// Restore the threshold last used on each overlay.
    PerOverlay,
}

impl OverlayThresholdSync {
    pub const ALL: [Self; 3] = [Self::CurrentValue, Self::MatchPValue, Self::PerOverlay];

    pub fn label(self) -> &'static str {
        match self {
            Self::CurrentValue => "Keep current slider value",
            Self::MatchPValue => "Match p-value when possible",
            Self::PerOverlay => "Remember each overlay separately",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::CurrentValue => {
                "Copy the current numeric threshold to the next overlay. Missing statistical metadata displays p -- and q --."
            }
            Self::MatchPValue => {
                "Convert through each overlay's stat metadata. If conversion is unavailable, keep the current numeric threshold."
            }
            Self::PerOverlay => "Restore the numeric threshold previously used on each overlay.",
        }
    }

    fn file_value(self) -> &'static str {
        match self {
            Self::CurrentValue => "CURRENT_VALUE",
            Self::MatchPValue => "P_VALUE",
            Self::PerOverlay => "PER_OVERLAY",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match unquote(value).trim().to_ascii_uppercase().as_str() {
            "CURRENT_VALUE" => Some(Self::CurrentValue),
            "P_VALUE" => Some(Self::MatchPValue),
            "PER_OVERLAY" => Some(Self::PerOverlay),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppPreferences {
    pub overlay_threshold_sync: OverlayThresholdSync,
}

impl AppPreferences {
    /// Load an existing preferences file, or exclusively create the complete
    /// self-documenting default file on first launch. `create_new` guarantees
    /// that an existing or concurrently created file is never overwritten.
    pub fn load_or_create(path: &Path) -> Result<(Self, bool)> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse_document(path, &text).map(|preferences| (preferences, false)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let preferences = Self::default();
                match preferences.create_new(path) {
                    Ok(()) => Ok((preferences, true)),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        Self::load(path).map(|preferences| (preferences, false))
                    }
                    Err(error) => Err(error).with_context(|| {
                        format!("failed to create default preferences {}", path.display())
                    }),
                }
            }
            Err(error) => {
                Err(error).with_context(|| format!("failed to read preferences {}", path.display()))
            }
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read preferences {}", path.display()));
            }
        };

        Self::parse_document(path, &text)
    }

    fn parse_document(path: &Path, text: &str) -> Result<Self> {
        let mut preferences = Self::default();
        for (line_number, raw_line) in text.lines().enumerate() {
            let line = preference_line(raw_line);
            if line.is_empty() || line.starts_with("***") {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                bail!(
                    "invalid preference at {}:{} (expected key = value)",
                    path.display(),
                    line_number + 1
                );
            };
            if matches!(
                key.trim(),
                "SUMARU_OverlayThresholdSync" | "overlay_threshold_sync"
            ) {
                preferences.overlay_threshold_sync = OverlayThresholdSync::parse(value)
                    .with_context(|| {
                        format!(
                            "invalid overlay_threshold_sync at {}:{}",
                            path.display(),
                            line_number + 1
                        )
                    })?;
            }
        }
        Ok(preferences)
    }

    fn create_new(self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(self.to_documented_text().as_bytes())?;
        file.sync_all()
    }

    pub fn save(self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create preferences directory {}",
                    parent.display()
                )
            })?;
        }
        let text = self.to_documented_text();
        let temporary = temporary_preferences_path(path);
        fs::write(&temporary, text).with_context(|| {
            format!(
                "failed to write temporary preferences {}",
                temporary.display()
            )
        })?;
        fs::rename(&temporary, path)
            .with_context(|| format!("failed to replace preferences {}", path.display()))?;
        Ok(())
    }

    fn to_documented_text(self) -> String {
        format!(
            "# SUMARU PREFERENCES\n\
#\n\
# This file is written by Sumaru's Settings > Preferences panel.\n\
# It follows the AFNI/SUMA ~/.sumarc style so every available setting is\n\
# documented where it is configured. Lines beginning with # or // are\n\
# comments. Unknown settings are ignored for forward compatibility.\n\
#\n\
***ENVIRONMENT\n\
// 000-SUMARU_OverlayThresholdSync:\n\
//     How the numeric threshold changes when selecting another overlay.\n\
//     CURRENT_VALUE: keep the current slider value. If the destination has\n\
//       no compatible statistical metadata, the viewer displays p -- and q --.\n\
//     P_VALUE: preserve the current p-value when both threshold columns have\n\
//       supported stat metadata; otherwise fall back to CURRENT_VALUE.\n\
//     PER_OVERLAY: restore the threshold last used on each overlay.\n\
//     default:   SUMARU_OverlayThresholdSync = CURRENT_VALUE\n\
   SUMARU_OverlayThresholdSync = {}\n",
            self.overlay_threshold_sync.file_value()
        )
    }
}

pub fn default_preferences_path() -> Option<PathBuf> {
    home_directory().map(|home| home.join(PREFERENCES_FILE_NAME))
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn temporary_preferences_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from(PREFERENCES_FILE_NAME));
    name.push(".tmp");
    path.with_file_name(name)
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(value)
}

fn preference_line(raw_line: &str) -> &str {
    let comment_start = [raw_line.find("//"), raw_line.find('#')]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(raw_line.len());
    raw_line[..comment_start].trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("sumaru-preferences-{label}-{nonce}"))
    }

    #[test]
    fn missing_file_uses_defaults() {
        let path = test_path("missing");
        assert_eq!(
            AppPreferences::load(&path).unwrap(),
            AppPreferences::default()
        );
    }

    #[test]
    fn first_load_creates_documented_defaults_without_overwriting() {
        let path = test_path("first-launch");
        let (preferences, created) = AppPreferences::load_or_create(&path).unwrap();
        assert!(created);
        assert_eq!(preferences, AppPreferences::default());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("***ENVIRONMENT"));
        assert!(text.contains("SUMARU_OverlayThresholdSync = CURRENT_VALUE"));

        fs::write(&path, "SUMARU_OverlayThresholdSync = PER_OVERLAY\n").unwrap();
        let (preferences, created) = AppPreferences::load_or_create(&path).unwrap();
        assert!(!created);
        assert_eq!(
            preferences.overlay_threshold_sync,
            OverlayThresholdSync::PerOverlay
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "SUMARU_OverlayThresholdSync = PER_OVERLAY\n"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn preferences_round_trip_and_ignore_unknown_keys() {
        let path = test_path("round-trip");
        AppPreferences {
            overlay_threshold_sync: OverlayThresholdSync::MatchPValue,
        }
        .save(&path)
        .unwrap();
        let mut text = fs::read_to_string(&path).unwrap();
        text.push_str("SUMARU_FutureSetting = YES\n");
        fs::write(&path, text).unwrap();
        assert_eq!(
            AppPreferences::load(&path).unwrap().overlay_threshold_sync,
            OverlayThresholdSync::MatchPValue
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_known_value_is_reported() {
        let path = test_path("invalid");
        fs::write(&path, "SUMARU_OverlayThresholdSync = nope\n").unwrap();
        assert!(AppPreferences::load(&path).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn documented_output_matches_sumarc_style() {
        let text = AppPreferences::default().to_documented_text();
        assert!(text.contains("***ENVIRONMENT"));
        assert!(text.contains("// 000-SUMARU_OverlayThresholdSync:"));
        assert!(text.contains("default:   SUMARU_OverlayThresholdSync = CURRENT_VALUE"));
        assert!(text.contains("SUMARU_OverlayThresholdSync = CURRENT_VALUE"));
    }

    #[test]
    fn sumarc_comments_and_legacy_key_are_both_accepted() {
        let path = test_path("compatible");
        fs::write(
            &path,
            "# SUMARU PREFERENCES\n***ENVIRONMENT\n// explanation\n\
             SUMARU_OverlayThresholdSync = P_VALUE // inline comment\n",
        )
        .unwrap();
        assert_eq!(
            AppPreferences::load(&path).unwrap().overlay_threshold_sync,
            OverlayThresholdSync::MatchPValue
        );

        fs::write(&path, "overlay_threshold_sync = \"per_overlay\"\n").unwrap();
        assert_eq!(
            AppPreferences::load(&path).unwrap().overlay_threshold_sync,
            OverlayThresholdSync::PerOverlay
        );
        let _ = fs::remove_file(path);
    }
}
