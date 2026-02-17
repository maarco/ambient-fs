use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Type of filesystem event
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Created,
    Modified,
    Deleted,
    Renamed,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EventType {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "created" => Ok(Self::Created),
            "modified" => Ok(Self::Modified),
            "deleted" => Ok(Self::Deleted),
            "renamed" => Ok(Self::Renamed),
            _ => Err(ParseError::InvalidEventType(s.to_string())),
        }
    }
}

/// Source of a filesystem change
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    User,
    AiAgent,
    Git,
    Build,
    Voice,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::AiAgent => "ai_agent",
            Self::Git => "git",
            Self::Build => "build",
            Self::Voice => "voice",
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Source {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "ai_agent" => Ok(Self::AiAgent),
            "git" => Ok(Self::Git),
            "build" => Ok(Self::Build),
            "voice" => Ok(Self::Voice),
            _ => Err(ParseError::InvalidSource(s.to_string())),
        }
    }
}

impl Default for Source {
    fn default() -> Self {
        Self::User
    }
}

/// A filesystem event with full attribution
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEvent {
    /// When the event occurred
    pub timestamp: DateTime<Utc>,
    /// What type of event
    pub event_type: EventType,
    /// File path relative to project root
    pub file_path: String,
    /// Project this event belongs to
    pub project_id: String,
    /// Who caused this change
    pub source: Source,
    /// Optional identifier for the source (e.g. chat_id, commit hash)
    pub source_id: Option<String>,
    /// Machine that generated this event
    pub machine_id: String,
    /// blake3 hash of file content (None for deletes)
    pub content_hash: Option<String>,
    /// Previous path (renames only)
    pub old_path: Option<String>,
}

impl FileEvent {
    pub fn new(
        event_type: EventType,
        file_path: impl Into<String>,
        project_id: impl Into<String>,
        machine_id: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            event_type,
            file_path: file_path.into(),
            project_id: project_id.into(),
            source: Source::default(),
            source_id: None,
            machine_id: machine_id.into(),
            content_hash: None,
            old_path: None,
        }
    }

    pub fn with_source(mut self, source: Source) -> Self {
        self.source = source;
        self
    }

    pub fn with_source_id(mut self, source_id: impl Into<String>) -> Self {
        self.source_id = Some(source_id.into());
        self
    }

    pub fn with_content_hash(mut self, hash: impl Into<String>) -> Self {
        self.content_hash = Some(hash.into());
        self
    }

    pub fn with_old_path(mut self, old_path: impl Into<String>) -> Self {
        self.old_path = Some(old_path.into());
        self
    }

    pub fn with_timestamp(mut self, ts: DateTime<Utc>) -> Self {
        self.timestamp = ts;
        self
    }

    /// Returns true if this event represents a rename
    pub fn is_rename(&self) -> bool {
        self.event_type == EventType::Renamed && self.old_path.is_some()
    }
}

/// Errors from parsing event/source strings
#[derive(Debug, Clone, thiserror::Error)]
pub enum ParseError {
    #[error("invalid event type: {0}")]
    InvalidEventType(String),
    #[error("invalid source: {0}")]
    InvalidSource(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn event_type_roundtrip() {
        for et in [
            EventType::Created,
            EventType::Modified,
            EventType::Deleted,
            EventType::Renamed,
        ] {
            let s = et.as_str();
            let parsed: EventType = s.parse().unwrap();
            assert_eq!(et, parsed);
            assert_eq!(et.to_string(), s);
        }
    }

    #[test]
    fn event_type_invalid() {
        let result: Result<EventType, _> = "bogus".parse();
        assert!(result.is_err());
    }

    #[test]
    fn source_roundtrip() {
        for src in [
            Source::User,
            Source::AiAgent,
            Source::Git,
            Source::Build,
            Source::Voice,
        ] {
            let s = src.as_str();
            let parsed: Source = s.parse().unwrap();
            assert_eq!(src, parsed);
            assert_eq!(src.to_string(), s);
        }
    }

    #[test]
    fn source_invalid() {
        let result: Result<Source, _> = "bogus".parse();
        assert!(result.is_err());
    }

    #[test]
    fn source_default_is_user() {
        assert_eq!(Source::default(), Source::User);
    }

    #[test]
    fn file_event_builder() {
        let event = FileEvent::new(EventType::Created, "src/main.rs", "proj-1", "machine-a")
            .with_source(Source::AiAgent)
            .with_source_id("chat_42")
            .with_content_hash("abc123");

        assert_eq!(event.event_type, EventType::Created);
        assert_eq!(event.file_path, "src/main.rs");
        assert_eq!(event.project_id, "proj-1");
        assert_eq!(event.machine_id, "machine-a");
        assert_eq!(event.source, Source::AiAgent);
        assert_eq!(event.source_id.as_deref(), Some("chat_42"));
        assert_eq!(event.content_hash.as_deref(), Some("abc123"));
        assert!(event.old_path.is_none());
        assert!(!event.is_rename());
    }

    #[test]
    fn file_event_rename() {
        let event = FileEvent::new(EventType::Renamed, "src/new.rs", "proj-1", "m1")
            .with_old_path("src/old.rs");

        assert!(event.is_rename());
        assert_eq!(event.old_path.as_deref(), Some("src/old.rs"));
    }

    #[test]
    fn file_event_rename_without_old_path_is_not_rename() {
        let event = FileEvent::new(EventType::Renamed, "src/new.rs", "proj-1", "m1");
        assert!(!event.is_rename());
    }

    #[test]
    fn file_event_serde_roundtrip() {
        let event = FileEvent::new(EventType::Modified, "src/lib.rs", "proj-1", "m1")
            .with_source(Source::Git)
            .with_source_id("abc123");

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: FileEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, deserialized);
    }

    #[test]
    fn event_type_serde() {
        let json = serde_json::to_string(&EventType::Created).unwrap();
        assert_eq!(json, "\"created\"");

        let parsed: EventType = serde_json::from_str("\"modified\"").unwrap();
        assert_eq!(parsed, EventType::Modified);
    }

    #[test]
    fn source_serde() {
        let json = serde_json::to_string(&Source::AiAgent).unwrap();
        assert_eq!(json, "\"ai_agent\"");

        let parsed: Source = serde_json::from_str("\"ai_agent\"").unwrap();
        assert_eq!(parsed, Source::AiAgent);
    }
}
