use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::event::Source;

/// How frequently a file has been changing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeFrequency {
    /// Changed in the last 10 minutes
    Hot,
    /// Changed in the last hour
    Warm,
    /// Not changed recently
    Cold,
}

impl ChangeFrequency {
    /// Determine frequency from time since last modification
    pub fn from_age(last_modified: DateTime<Utc>, now: DateTime<Utc>) -> Self {
        let age = now.signed_duration_since(last_modified);
        if age < Duration::minutes(10) {
            Self::Hot
        } else if age < Duration::hours(1) {
            Self::Warm
        } else {
            Self::Cold
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Warm => "warm",
            Self::Cold => "cold",
        }
    }
}

impl std::fmt::Display for ChangeFrequency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Aggregated awareness state for a single file
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAwareness {
    pub file_path: String,
    pub project_id: String,
    pub last_modified: DateTime<Utc>,
    pub modified_by: Source,
    /// Human-readable label for the modifier (e.g. chat title, commit msg)
    pub modified_by_label: Option<String>,
    /// Currently active agent working on this file (if any)
    pub active_agent: Option<String>,
    /// Number of chat conversations that reference this file
    pub chat_references: u32,
    /// Number of TODO/FIXME/HACK comments
    pub todo_count: u32,
    /// Number of lint hints/warnings
    pub lint_hints: u32,
    /// Total line count
    pub line_count: u32,
    /// How frequently this file changes
    pub change_frequency: ChangeFrequency,
}

impl FileAwareness {
    /// Create a minimal awareness entry (e.g. from a single event)
    pub fn from_event_minimal(
        file_path: impl Into<String>,
        project_id: impl Into<String>,
        modified_at: DateTime<Utc>,
        source: Source,
    ) -> Self {
        let now = Utc::now();
        Self {
            file_path: file_path.into(),
            project_id: project_id.into(),
            last_modified: modified_at,
            modified_by: source,
            modified_by_label: None,
            active_agent: None,
            chat_references: 0,
            todo_count: 0,
            lint_hints: 0,
            line_count: 0,
            change_frequency: ChangeFrequency::from_age(modified_at, now),
        }
    }

    /// How many seconds ago this file was modified
    pub fn age_secs(&self) -> i64 {
        Utc::now().signed_duration_since(self.last_modified).num_seconds()
    }

    /// Human-readable relative time (e.g. "2m", "1h", "3d")
    pub fn relative_time(&self) -> String {
        let secs = self.age_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else if secs < 86400 {
            format!("{}h", secs / 3600)
        } else {
            format!("{}d", secs / 86400)
        }
    }

    /// Recompute change_frequency based on current time
    pub fn refresh_frequency(&mut self) {
        self.change_frequency = ChangeFrequency::from_age(self.last_modified, Utc::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn change_frequency_hot() {
        let now = Utc::now();
        let modified = now - Duration::minutes(5);
        assert_eq!(ChangeFrequency::from_age(modified, now), ChangeFrequency::Hot);
    }

    #[test]
    fn change_frequency_warm() {
        let now = Utc::now();
        let modified = now - Duration::minutes(30);
        assert_eq!(ChangeFrequency::from_age(modified, now), ChangeFrequency::Warm);
    }

    #[test]
    fn change_frequency_cold() {
        let now = Utc::now();
        let modified = now - Duration::hours(2);
        assert_eq!(ChangeFrequency::from_age(modified, now), ChangeFrequency::Cold);
    }

    #[test]
    fn change_frequency_boundary_10min() {
        let now = Utc::now();
        // exactly 10 min = warm (not hot)
        let modified = now - Duration::minutes(10);
        assert_eq!(ChangeFrequency::from_age(modified, now), ChangeFrequency::Warm);
    }

    #[test]
    fn change_frequency_boundary_1h() {
        let now = Utc::now();
        // exactly 1h = cold (not warm)
        let modified = now - Duration::hours(1);
        assert_eq!(ChangeFrequency::from_age(modified, now), ChangeFrequency::Cold);
    }

    #[test]
    fn change_frequency_display() {
        assert_eq!(ChangeFrequency::Hot.as_str(), "hot");
        assert_eq!(ChangeFrequency::Warm.as_str(), "warm");
        assert_eq!(ChangeFrequency::Cold.as_str(), "cold");
    }

    #[test]
    fn file_awareness_from_event_minimal() {
        let now = Utc::now();
        let awareness =
            FileAwareness::from_event_minimal("src/main.rs", "proj-1", now, Source::AiAgent);

        assert_eq!(awareness.file_path, "src/main.rs");
        assert_eq!(awareness.project_id, "proj-1");
        assert_eq!(awareness.modified_by, Source::AiAgent);
        assert_eq!(awareness.change_frequency, ChangeFrequency::Hot);
        assert_eq!(awareness.chat_references, 0);
        assert_eq!(awareness.todo_count, 0);
        assert!(awareness.active_agent.is_none());
    }

    #[test]
    fn relative_time_seconds() {
        let now = Utc::now();
        let awareness =
            FileAwareness::from_event_minimal("f.rs", "p", now - Duration::seconds(30), Source::User);
        let rt = awareness.relative_time();
        assert!(rt.ends_with('s'), "expected seconds, got: {}", rt);
    }

    #[test]
    fn relative_time_minutes() {
        let now = Utc::now();
        let awareness =
            FileAwareness::from_event_minimal("f.rs", "p", now - Duration::minutes(5), Source::User);
        let rt = awareness.relative_time();
        assert!(rt.ends_with('m'), "expected minutes, got: {}", rt);
    }

    #[test]
    fn relative_time_hours() {
        let now = Utc::now();
        let awareness =
            FileAwareness::from_event_minimal("f.rs", "p", now - Duration::hours(3), Source::User);
        let rt = awareness.relative_time();
        assert!(rt.ends_with('h'), "expected hours, got: {}", rt);
    }

    #[test]
    fn relative_time_days() {
        let now = Utc::now();
        let awareness =
            FileAwareness::from_event_minimal("f.rs", "p", now - Duration::days(2), Source::User);
        let rt = awareness.relative_time();
        assert!(rt.ends_with('d'), "expected days, got: {}", rt);
    }

    #[test]
    fn file_awareness_serde_roundtrip() {
        let awareness = FileAwareness {
            file_path: "src/lib.rs".to_string(),
            project_id: "proj-1".to_string(),
            last_modified: Utc::now(),
            modified_by: Source::Git,
            modified_by_label: Some("Fix auth".to_string()),
            active_agent: None,
            chat_references: 3,
            todo_count: 1,
            lint_hints: 0,
            line_count: 142,
            change_frequency: ChangeFrequency::Warm,
        };

        let json = serde_json::to_string(&awareness).unwrap();
        let deserialized: FileAwareness = serde_json::from_str(&json).unwrap();
        assert_eq!(awareness, deserialized);
    }
}
