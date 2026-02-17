// Agent activity tracking via JSONL file watching
// TDD: Tests FIRST, then implementation

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Parsed agent activity from a JSONL line
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentActivity {
    /// Unix timestamp (seconds)
    pub ts: i64,
    /// Agent identifier
    pub agent: String,
    /// What the agent is doing
    pub action: String,
    /// Relative file path being acted on
    pub file: String,
    /// Optional project identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Which AI tool (claude-code, cursor, etc)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Session/conversation id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Human-readable description of what the agent is doing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Line numbers being edited
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<u32>>,
    /// 0.0-1.0 confidence level
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// True when agent is finished with this file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
}

impl PartialEq for AgentActivity {
    fn eq(&self, other: &Self) -> bool {
        self.ts == other.ts
            && self.agent == other.agent
            && self.action == other.action
            && self.file == other.file
            && self.project == other.project
            && self.tool == other.tool
            && self.session == other.session
            && self.intent == other.intent
            && self.lines == other.lines
            // skip confidence in comparison (f64 doesn't impl Eq)
            && self.done == other.done
    }
}

impl AgentActivity {
    /// Minimum required fields constructor
    pub fn new(ts: i64, agent: impl Into<String>, action: impl Into<String>, file: impl Into<String>) -> Self {
        Self {
            ts,
            agent: agent.into(),
            action: action.into(),
            file: file.into(),
            project: None,
            tool: None,
            session: None,
            intent: None,
            lines: None,
            confidence: None,
            done: None,
        }
    }

    /// Check if this activity marks a file as done
    pub fn is_done(&self) -> bool {
        self.done.unwrap_or(false)
    }

    /// Get the timestamp as DateTime
    pub fn timestamp(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(self.ts, 0).unwrap_or_else(|| Utc::now())
    }
}

/// Per-agent tracking state
#[derive(Debug, Clone)]
pub struct AgentState {
    /// Agent identifier
    pub agent_id: String,
    /// Currently active files
    pub files: Vec<String>,
    /// Last activity timestamp
    pub last_seen: DateTime<Utc>,
    /// Current intent/description
    pub intent: Option<String>,
    /// Tool being used
    pub tool: Option<String>,
    /// Session identifier
    pub session: Option<String>,
}

impl AgentState {
    /// Create a new agent state
    pub fn new(agent_id: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            agent_id: agent_id.into(),
            files: Vec::new(),
            last_seen: now,
            intent: None,
            tool: None,
            session: None,
        }
    }

    /// Check if this agent is stale (not seen within timeout)
    pub fn is_stale(&self, timeout: Duration) -> bool {
        Utc::now() - self.last_seen > timeout
    }
}

/// Tracks agent activity from JSONL files
///
/// Watches registered directories for .jsonl files,
/// parses activity lines, and maintains per-agent state.
#[derive(Debug, Clone)]
pub struct AgentTracker {
    /// Per-agent state
    agents: Arc<RwLock<HashMap<String, AgentState>>>,
    /// Timeout after which agents are considered stale
    stale_timeout: Duration,
    /// File path -> reference count (how many times referenced in chat)
    file_references: Arc<RwLock<HashMap<String, u32>>>,
}

impl AgentTracker {
    /// Create a new AgentTracker
    pub fn new(stale_timeout: Duration) -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            stale_timeout,
            file_references: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with default 5-minute stale timeout
    pub fn with_default_timeout() -> Self {
        Self::new(Duration::seconds(300))
    }

    /// Parse a JSONL line into AgentActivity
    ///
    /// Returns None if the line is not valid JSON or missing required fields.
    pub fn process_line(&self, line: &str) -> Option<AgentActivity> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        // Parse JSON
        let activity: AgentActivity = serde_json::from_str(trimmed).ok()?;

        // Validate required fields
        if activity.agent.is_empty() || activity.action.is_empty() || activity.file.is_empty() {
            return None;
        }

        Some(activity)
    }

    /// Update agent state from an activity
    pub async fn update_from_activity(&self, activity: &AgentActivity) {
        let mut agents = self.agents.write().await;
        let agent_id = activity.agent.clone();

        // Get or create agent state
        let state = agents.entry(agent_id.clone())
            .or_insert_with(|| AgentState::new(&agent_id));

        // Update timestamp
        state.last_seen = activity.timestamp();
        state.intent = activity.intent.clone();
        state.tool = activity.tool.clone();
        state.session = activity.session.clone();

        // Update file list
        let file = activity.file.clone();

        if activity.is_done() {
            // Remove file from active list
            state.files.retain(|f| f != &file);
        } else {
            // Add file if not already present
            if !state.files.contains(&file) {
                state.files.push(file.clone());
            }
        }
        drop(agents);

        // Increment file reference count
        let mut refs = self.file_references.write().await;
        *refs.entry(file.clone()).or_insert(0) += 1;
    }

    /// Get the active agent for a specific file
    ///
    /// Returns the most recently seen agent that has this file in its active list.
    /// Skips stale agents.
    pub async fn get_active_agent(&self, file_path: &str) -> Option<String> {
        let agents = self.agents.read().await;

        let mut best_agent: Option<(String, DateTime<Utc>)> = None;

        for (agent_id, state) in agents.iter() {
            // Skip stale agents
            if state.is_stale(self.stale_timeout) {
                continue;
            }

            // Check if agent has this file
            if state.files.contains(&file_path.to_string()) {
                match &best_agent {
                    None => {
                        best_agent = Some((agent_id.clone(), state.last_seen));
                    }
                    Some((_, last_seen)) => {
                        if state.last_seen > *last_seen {
                            best_agent = Some((agent_id.clone(), state.last_seen));
                        }
                    }
                }
            }
        }

        best_agent.map(|(agent_id, _)| agent_id)
    }

    /// Get all non-stale agents
    pub async fn get_all_agents(&self) -> Vec<AgentState> {
        let agents = self.agents.read().await;
        agents.values()
            .filter(|state| !state.is_stale(self.stale_timeout))
            .cloned()
            .collect()
    }

    /// Remove agents not seen within the stale timeout
    ///
    /// Also resets reference counts for files that are only referenced
    /// by stale agents (files with at least one fresh agent reference are preserved).
    pub async fn prune_stale(&self) -> usize {
        let mut agents = self.agents.write().await;

        // Collect files that stale agents have referenced
        let mut stale_files: HashMap<String, u32> = HashMap::new();
        let mut fresh_files: HashMap<String, u32> = HashMap::new();

        for (_agent_id, state) in agents.iter() {
            if state.is_stale(self.stale_timeout) {
                // Count files referenced by stale agents
                for file in &state.files {
                    *stale_files.entry(file.clone()).or_insert(0) += 1;
                }
            } else {
                // Count files referenced by fresh agents
                for file in &state.files {
                    *fresh_files.entry(file.clone()).or_insert(0) += 1;
                }
            }
        }

        // Remove stale agents
        let before = agents.len();
        agents.retain(|_, state| !state.is_stale(self.stale_timeout));
        let pruned_count = before - agents.len();
        drop(agents);

        // Reset reference counts for files only referenced by stale agents
        if !stale_files.is_empty() {
            let mut refs = self.file_references.write().await;
            for (file, _) in stale_files {
                // Only reset if no fresh agent also references this file
                if !fresh_files.contains_key(&file) {
                    refs.remove(&file);
                }
            }
        }

        pruned_count
    }

    /// Calculate pass rate for a batch of lines
    ///
    /// Returns the ratio of parseable lines to total lines.
    pub fn calculate_pass_rate<'a>(&self, lines: &[&'a str]) -> f64 {
        if lines.is_empty() {
            return 1.0;
        }

        let valid = lines.iter()
            .filter(|line| self.process_line(line).is_some())
            .count();

        valid as f64 / lines.len() as f64
    }

    /// Get agents count (including stale)
    pub async fn agent_count(&self) -> usize {
        self.agents.read().await.len()
    }

    /// Get active (non-stale) agents count
    pub async fn active_agent_count(&self) -> usize {
        let agents = self.agents.read().await;
        agents.values()
            .filter(|state| !state.is_stale(self.stale_timeout))
            .count()
    }

    /// Get the reference count for a specific file
    ///
    /// Returns the number of times this file has been referenced
    /// in agent activity (chat sessions).
    pub async fn get_reference_count(&self, file_path: &str) -> u32 {
        let refs = self.file_references.read().await;
        refs.get(file_path).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== AgentActivity ==========

    #[test]
    fn activity_new_creates_minimal() {
        let activity = AgentActivity::new(1708099200, "agent-1", "edit", "src/main.rs");

        assert_eq!(activity.ts, 1708099200);
        assert_eq!(activity.agent, "agent-1");
        assert_eq!(activity.action, "edit");
        assert_eq!(activity.file, "src/main.rs");
        assert!(activity.project.is_none());
        assert!(activity.intent.is_none());
    }

    #[test]
    fn activity_serialize_full() {
        let activity = AgentActivity {
            ts: 1708099200,
            agent: "claude-7".to_string(),
            action: "edit".to_string(),
            file: "src/auth.rs".to_string(),
            project: Some("ambient-fs".to_string()),
            tool: Some("claude-code".to_string()),
            session: Some("session-42".to_string()),
            intent: Some("fixing auth bypass".to_string()),
            lines: Some(vec![42, 67]),
            confidence: Some(0.95),
            done: Some(false),
        };

        let json = serde_json::to_string(&activity).unwrap();
        let parsed: AgentActivity = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed, activity);
        assert_eq!(parsed.intent, Some("fixing auth bypass".to_string()));
        assert_eq!(parsed.lines, Some(vec![42, 67]));
    }

    #[test]
    fn activity_serialize_minimal() {
        let activity = AgentActivity::new(1708099200, "agent-1", "read", "README.md");

        let json = serde_json::to_string(&activity).unwrap();
        let parsed: AgentActivity = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.agent, "agent-1");
        assert!(parsed.intent.is_none());
        assert!(parsed.lines.is_none());
    }

    #[test]
    fn activity_is_done_checks_flag() {
        let mut activity = AgentActivity::new(1708099200, "agent-1", "edit", "src/foo.rs");
        assert!(!activity.is_done());

        activity.done = Some(true);
        assert!(activity.is_done());

        activity.done = Some(false);
        assert!(!activity.is_done());
    }

    #[test]
    fn activity_timestamp_parses_unix_ts() {
        let activity = AgentActivity::new(1708099200, "agent-1", "edit", "src/foo.rs");
        let ts = activity.timestamp();

        assert_eq!(ts.timestamp(), 1708099200);
    }

    // ========== AgentState ==========

    #[test]
    fn state_new_creates_empty() {
        let state = AgentState::new("agent-1");

        assert_eq!(state.agent_id, "agent-1");
        assert!(state.files.is_empty());
        assert!(state.intent.is_none());
        assert!(state.tool.is_none());
    }

    #[test]
    fn state_is_stale_timeout() {
        let mut state = AgentState::new("agent-1");
        let timeout = Duration::seconds(300);

        // Fresh agent is not stale
        assert!(!state.is_stale(timeout));

        // Old agent is stale
        state.last_seen = Utc::now() - Duration::seconds(301);
        assert!(state.is_stale(timeout));
    }

    // ========== AgentTracker::new ==========

    #[test]
    fn new_creates_tracker() {
        let tracker = AgentTracker::new(Duration::seconds(60));
        assert_eq!(tracker.stale_timeout, Duration::seconds(60));
    }

    #[test]
    fn with_default_timeout_uses_5_minutes() {
        let tracker = AgentTracker::with_default_timeout();
        assert_eq!(tracker.stale_timeout, Duration::seconds(300));
    }

    // ========== AgentTracker::process_line ==========

    #[test]
    fn process_line_valid_json_returns_activity() {
        let tracker = AgentTracker::with_default_timeout();
        let line = r#"{"ts":1708099200,"agent":"agent-1","action":"edit","file":"src/main.rs"}"#;

        let activity = tracker.process_line(line);

        assert!(activity.is_some());
        let act = activity.unwrap();
        assert_eq!(act.agent, "agent-1");
        assert_eq!(act.action, "edit");
        assert_eq!(act.file, "src/main.rs");
    }

    #[test]
    fn process_line_with_optional_fields() {
        let tracker = AgentTracker::with_default_timeout();
        let line = r#"{"ts":1708099200,"agent":"agent-1","action":"edit","file":"src/main.rs","intent":"fix bug","tool":"claude-code"}"#;

        let activity = tracker.process_line(line);

        assert!(activity.is_some());
        let act = activity.unwrap();
        assert_eq!(act.intent, Some("fix bug".to_string()));
        assert_eq!(act.tool, Some("claude-code".to_string()));
    }

    #[test]
    fn process_line_invalid_json_returns_none() {
        let tracker = AgentTracker::with_default_timeout();
        let activity = tracker.process_line("not json");
        assert!(activity.is_none());
    }

    #[test]
    fn process_line_empty_returns_none() {
        let tracker = AgentTracker::with_default_timeout();
        assert!(tracker.process_line("").is_none());
        assert!(tracker.process_line("   ").is_none());
        assert!(tracker.process_line("\n").is_none());
    }

    #[test]
    fn process_line_missing_required_field_returns_none() {
        let tracker = AgentTracker::with_default_timeout();

        // Missing agent
        let line1 = r#"{"ts":1708099200,"action":"edit","file":"src/main.rs"}"#;
        assert!(tracker.process_line(line1).is_none());

        // Missing action
        let line2 = r#"{"ts":1708099200,"agent":"agent-1","file":"src/main.rs"}"#;
        assert!(tracker.process_line(line2).is_none());

        // Missing file
        let line3 = r#"{"ts":1708099200,"agent":"agent-1","action":"edit"}"#;
        assert!(tracker.process_line(line3).is_none());
    }

    #[test]
    fn process_line_empty_string_fields_returns_none() {
        let tracker = AgentTracker::with_default_timeout();

        // Empty agent
        let line1 = r#"{"ts":1708099200,"agent":"","action":"edit","file":"src/main.rs"}"#;
        assert!(tracker.process_line(line1).is_none());

        // Empty file
        let line2 = r#"{"ts":1708099200,"agent":"agent-1","action":"edit","file":""}"#;
        assert!(tracker.process_line(line2).is_none());
    }

    #[test]
    fn process_line_with_whitespace() {
        let tracker = AgentTracker::with_default_timeout();
        let line = r#"  {"ts":1708099200,"agent":"agent-1","action":"edit","file":"src/main.rs"}  "#;

        let activity = tracker.process_line(line);
        assert!(activity.is_some());
    }

    // ========== AgentTracker::update_from_activity ==========

    #[tokio::test]
    async fn update_creates_new_agent() {
        let tracker = AgentTracker::with_default_timeout();
        let activity = AgentActivity::new(1708099200, "new-agent", "edit", "src/foo.rs");

        tracker.update_from_activity(&activity).await;

        assert_eq!(tracker.agent_count().await, 1);
    }

    #[tokio::test]
    async fn update_adds_file_to_agent() {
        let tracker = AgentTracker::with_default_timeout();
        let activity = AgentActivity::new(1708099200, "agent-1", "edit", "src/foo.rs");

        tracker.update_from_activity(&activity).await;

        let agents = tracker.agents.read().await;
        let state = agents.get("agent-1").unwrap();
        assert!(state.files.contains(&"src/foo.rs".to_string()));
    }

    #[tokio::test]
    async fn update_updates_last_seen() {
        let tracker = AgentTracker::with_default_timeout();
        let ts = 1708099200;
        let activity = AgentActivity::new(ts, "agent-1", "edit", "src/foo.rs");

        tracker.update_from_activity(&activity).await;

        let agents = tracker.agents.read().await;
        let state = agents.get("agent-1").unwrap();
        assert_eq!(state.last_seen.timestamp(), ts);
    }

    #[tokio::test]
    async fn update_copies_intent_and_tool() {
        let tracker = AgentTracker::with_default_timeout();
        let mut activity = AgentActivity::new(1708099200, "agent-1", "edit", "src/foo.rs");
        activity.intent = Some("fix the bug".to_string());
        activity.tool = Some("cursor".to_string());
        activity.session = Some("sess-1".to_string());

        tracker.update_from_activity(&activity).await;

        let agents = tracker.agents.read().await;
        let state = agents.get("agent-1").unwrap();
        assert_eq!(state.intent, Some("fix the bug".to_string()));
        assert_eq!(state.tool, Some("cursor".to_string()));
        assert_eq!(state.session, Some("sess-1".to_string()));
    }

    #[tokio::test]
    async fn update_done_removes_file_from_active() {
        let tracker = AgentTracker::with_default_timeout();

        // First activity adds file
        let activity1 = AgentActivity::new(1708099200, "agent-1", "edit", "src/foo.rs");
        tracker.update_from_activity(&activity1).await;

        let agents = tracker.agents.read().await;
        assert!(agents.get("agent-1").unwrap().files.contains(&"src/foo.rs".to_string()));
        drop(agents);

        // Done activity removes file
        let mut activity2 = AgentActivity::new(1708099300, "agent-1", "edit", "src/foo.rs");
        activity2.done = Some(true);
        tracker.update_from_activity(&activity2).await;

        let agents = tracker.agents.read().await;
        assert!(!agents.get("agent-1").unwrap().files.contains(&"src/foo.rs".to_string()));
    }

    #[tokio::test]
    async fn update_does_not_duplicate_files() {
        let tracker = AgentTracker::with_default_timeout();
        let activity = AgentActivity::new(1708099200, "agent-1", "edit", "src/foo.rs");

        tracker.update_from_activity(&activity).await;
        tracker.update_from_activity(&activity).await;
        tracker.update_from_activity(&activity).await;

        let agents = tracker.agents.read().await;
        let state = agents.get("agent-1").unwrap();
        assert_eq!(state.files.len(), 1);
    }

    // ========== AgentTracker::get_active_agent ==========

    #[tokio::test]
    async fn get_active_agent_returns_none_for_unknown_file() {
        let tracker = AgentTracker::with_default_timeout();
        assert!(tracker.get_active_agent("unknown.rs").await.is_none());
    }

    #[tokio::test]
    async fn get_active_agent_returns_agent_with_file() {
        let tracker = AgentTracker::with_default_timeout();
        let now = Utc::now().timestamp();
        let activity = AgentActivity::new(now, "agent-1", "edit", "src/foo.rs");
        tracker.update_from_activity(&activity).await;

        let agent = tracker.get_active_agent("src/foo.rs").await;
        assert_eq!(agent, Some("agent-1".to_string()));
    }

    #[tokio::test]
    async fn get_active_agent_skips_stale_agents() {
        let tracker = AgentTracker::new(Duration::seconds(60));

        // Add an agent with old timestamp
        let old_ts = (Utc::now() - Duration::seconds(61)).timestamp();
        let activity = AgentActivity::new(old_ts, "old-agent", "edit", "src/foo.rs");
        tracker.update_from_activity(&activity).await;

        // Should not find the stale agent
        assert!(tracker.get_active_agent("src/foo.rs").await.is_none());
    }

    #[tokio::test]
    async fn get_active_agent_returns_most_recent() {
        let tracker = AgentTracker::with_default_timeout();
        let now = Utc::now().timestamp();

        // Agent 1 on file
        let activity1 = AgentActivity::new(now, "agent-1", "edit", "src/foo.rs");
        tracker.update_from_activity(&activity1).await;

        // Agent 2 on same file, more recent
        let activity2 = AgentActivity::new(now + 100, "agent-2", "edit", "src/foo.rs");
        tracker.update_from_activity(&activity2).await;

        // Agent 2 should be returned (more recent)
        let agent = tracker.get_active_agent("src/foo.rs").await;
        assert_eq!(agent, Some("agent-2".to_string()));
    }

    #[tokio::test]
    async fn get_active_agent_multiple_files_per_agent() {
        let tracker = AgentTracker::with_default_timeout();
        let now = Utc::now().timestamp();

        let activity1 = AgentActivity::new(now, "agent-1", "edit", "src/foo.rs");
        let activity2 = AgentActivity::new(now, "agent-1", "edit", "src/bar.rs");
        tracker.update_from_activity(&activity1).await;
        tracker.update_from_activity(&activity2).await;

        assert_eq!(tracker.get_active_agent("src/foo.rs").await, Some("agent-1".to_string()));
        assert_eq!(tracker.get_active_agent("src/bar.rs").await, Some("agent-1".to_string()));
    }

    // ========== AgentTracker::get_all_agents ==========

    #[tokio::test]
    async fn get_all_agents_returns_empty_initially() {
        let tracker = AgentTracker::with_default_timeout();
        assert!(tracker.get_all_agents().await.is_empty());
    }

    #[tokio::test]
    async fn get_all_agents_excludes_stale() {
        let tracker = AgentTracker::new(Duration::seconds(60));

        // Add stale agent
        let old_ts = (Utc::now() - Duration::seconds(61)).timestamp();
        let activity1 = AgentActivity::new(old_ts, "stale-agent", "edit", "src/old.rs");
        tracker.update_from_activity(&activity1).await;

        // Add fresh agent
        let now = Utc::now().timestamp();
        let activity2 = AgentActivity::new(now, "fresh-agent", "edit", "src/new.rs");
        tracker.update_from_activity(&activity2).await;

        let agents = tracker.get_all_agents().await;
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_id, "fresh-agent");
    }

    // ========== AgentTracker::prune_stale ==========

    #[tokio::test]
    async fn prune_stale_removes_old_agents() {
        let tracker = AgentTracker::new(Duration::seconds(60));
        let now = Utc::now().timestamp();

        // Add fresh agent
        let activity1 = AgentActivity::new(now, "fresh", "edit", "src/a.rs");
        tracker.update_from_activity(&activity1).await;

        // Add stale agent
        let old_ts = (Utc::now() - Duration::seconds(61)).timestamp();
        let activity2 = AgentActivity::new(old_ts, "stale", "edit", "src/b.rs");
        tracker.update_from_activity(&activity2).await;

        assert_eq!(tracker.agent_count().await, 2);

        let pruned = tracker.prune_stale().await;
        assert_eq!(pruned, 1);
        assert_eq!(tracker.agent_count().await, 1);
    }

    #[tokio::test]
    async fn prune_stale_returns_zero_when_no_stale() {
        let tracker = AgentTracker::with_default_timeout();
        let now = Utc::now().timestamp();
        let activity = AgentActivity::new(now, "agent-1", "edit", "src/a.rs");
        tracker.update_from_activity(&activity).await;

        let pruned = tracker.prune_stale().await;
        assert_eq!(pruned, 0);
    }

    // ========== AgentTracker::calculate_pass_rate ==========

    #[test]
    fn calculate_pass_rate_all_valid() {
        let tracker = AgentTracker::with_default_timeout();
        let lines = vec![
            r#"{"ts":1708099200,"agent":"a","action":"edit","file":"src/a.rs"}"#,
            r#"{"ts":1708099201,"agent":"b","action":"read","file":"src/b.rs"}"#,
            r#"{"ts":1708099202,"agent":"c","action":"edit","file":"src/c.rs"}"#,
        ];

        let rate = tracker.calculate_pass_rate(&lines);
        assert_eq!(rate, 1.0);
    }

    #[test]
    fn calculate_pass_rate_none_valid() {
        let tracker = AgentTracker::with_default_timeout();
        let lines = vec![
            "not json",
            "also not json",
            "definitely not json",
        ];

        let rate = tracker.calculate_pass_rate(&lines);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn calculate_pass_rate_mixed() {
        let tracker = AgentTracker::with_default_timeout();
        let lines = vec![
            r#"{"ts":1708099200,"agent":"a","action":"edit","file":"src/a.rs"}"#,
            "not json",
            r#"{"ts":1708099201,"agent":"b","action":"read","file":"src/b.rs"}"#,
            "also not json",
        ];

        let rate = tracker.calculate_pass_rate(&lines);
        assert_eq!(rate, 0.5);
    }

    #[test]
    fn calculate_pass_rate_empty_returns_one() {
        let tracker = AgentTracker::with_default_timeout();
        let lines: Vec<&str> = vec![];

        let rate = tracker.calculate_pass_rate(&lines);
        assert_eq!(rate, 1.0);
    }

    #[test]
    fn calculate_pass_rate_with_empty_lines() {
        let tracker = AgentTracker::with_default_timeout();
        let lines = vec![
            "",
            "   ",
            r#"{"ts":1708099200,"agent":"a","action":"edit","file":"src/a.rs"}"#,
        ];

        let rate = tracker.calculate_pass_rate(&lines);
        assert_eq!(rate, 1.0 / 3.0);
    }

    // ========== AgentTracker::agent_count ==========

    #[tokio::test]
    async fn agent_count_returns_zero_initially() {
        let tracker = AgentTracker::with_default_timeout();
        assert_eq!(tracker.agent_count().await, 0);
    }

    #[tokio::test]
    async fn agent_count_includes_stale() {
        let tracker = AgentTracker::new(Duration::seconds(60));

        let activity = AgentActivity::new(1708099200, "agent-1", "edit", "src/a.rs");
        tracker.update_from_activity(&activity).await;

        // Before stale timeout
        assert_eq!(tracker.agent_count().await, 1);

        // After stale timeout (simulated by setting old timestamp)
        let old_ts = (Utc::now() - Duration::seconds(61)).timestamp();
        let activity2 = AgentActivity::new(old_ts, "agent-2", "edit", "src/b.rs");
        tracker.update_from_activity(&activity2).await;

        // Both agents still counted
        assert_eq!(tracker.agent_count().await, 2);
    }

    // ========== AgentTracker::active_agent_count ==========

    #[tokio::test]
    async fn active_agent_count_excludes_stale() {
        let tracker = AgentTracker::new(Duration::seconds(60));
        let now = Utc::now().timestamp();

        // Fresh agent
        let activity1 = AgentActivity::new(now, "fresh", "edit", "src/a.rs");
        tracker.update_from_activity(&activity1).await;

        // Stale agent
        let old_ts = (Utc::now() - Duration::seconds(61)).timestamp();
        let activity2 = AgentActivity::new(old_ts, "stale", "edit", "src/b.rs");
        tracker.update_from_activity(&activity2).await;

        assert_eq!(tracker.active_agent_count().await, 1);
    }

    // ========== Integration scenarios ==========

    #[tokio::test]
    async fn multiple_agents_different_files() {
        let tracker = AgentTracker::with_default_timeout();
        let now = Utc::now().timestamp();

        // Agent 1 working on auth.rs
        let activity1 = AgentActivity::new(now, "agent-1", "edit", "src/auth.rs");
        tracker.update_from_activity(&activity1).await;

        // Agent 2 working on main.rs
        let activity2 = AgentActivity::new(now + 50, "agent-2", "edit", "src/main.rs");
        tracker.update_from_activity(&activity2).await;

        // Agent 3 working on models.rs
        let activity3 = AgentActivity::new(now + 100, "agent-3", "edit", "src/models.rs");
        tracker.update_from_activity(&activity3).await;

        assert_eq!(tracker.active_agent_count().await, 3);
        assert_eq!(tracker.get_active_agent("src/auth.rs").await, Some("agent-1".to_string()));
        assert_eq!(tracker.get_active_agent("src/main.rs").await, Some("agent-2".to_string()));
        assert_eq!(tracker.get_active_agent("src/models.rs").await, Some("agent-3".to_string()));
    }

    #[tokio::test]
    async fn agent_completes_file_work() {
        let tracker = AgentTracker::with_default_timeout();
        let now = Utc::now().timestamp();

        // Start work on file
        let activity1 = AgentActivity::new(now, "agent-1", "edit", "src/auth.rs");
        tracker.update_from_activity(&activity1).await;
        assert_eq!(tracker.get_active_agent("src/auth.rs").await, Some("agent-1".to_string()));

        // Complete work
        let mut activity2 = AgentActivity::new(now + 100, "agent-1", "edit", "src/auth.rs");
        activity2.done = Some(true);
        tracker.update_from_activity(&activity2).await;

        // No longer active
        assert!(tracker.get_active_agent("src/auth.rs").await.is_none());
    }

    #[tokio::test]
    async fn agent_transitions_between_files() {
        let tracker = AgentTracker::with_default_timeout();
        let now = Utc::now().timestamp();

        // Work on file A
        let activity1 = AgentActivity::new(now, "agent-1", "edit", "src/a.rs");
        tracker.update_from_activity(&activity1).await;

        // Work on file B
        let activity2 = AgentActivity::new(now + 50, "agent-1", "edit", "src/b.rs");
        tracker.update_from_activity(&activity2).await;

        // Both files active
        assert_eq!(tracker.get_active_agent("src/a.rs").await, Some("agent-1".to_string()));
        assert_eq!(tracker.get_active_agent("src/b.rs").await, Some("agent-1".to_string()));

        // Complete file A
        let mut activity3 = AgentActivity::new(now + 100, "agent-1", "edit", "src/a.rs");
        activity3.done = Some(true);
        tracker.update_from_activity(&activity3).await;

        // Only file B still active
        assert!(tracker.get_active_agent("src/a.rs").await.is_none());
        assert_eq!(tracker.get_active_agent("src/b.rs").await, Some("agent-1".to_string()));
    }

    // ========== AgentTracker::get_reference_count ==========

    #[tokio::test]
    async fn get_reference_count_returns_zero_for_unknown_file() {
        let tracker = AgentTracker::with_default_timeout();
        assert_eq!(tracker.get_reference_count("unknown.rs").await, 0);
    }

    #[tokio::test]
    async fn get_reference_count_increments_on_activity() {
        let tracker = AgentTracker::with_default_timeout();
        let now = Utc::now().timestamp();

        // First activity
        let activity1 = AgentActivity::new(now, "agent-1", "edit", "src/main.rs");
        tracker.update_from_activity(&activity1).await;

        assert_eq!(tracker.get_reference_count("src/main.rs").await, 1);
    }

    #[tokio::test]
    async fn get_reference_count_accumulates_multiple_activities() {
        let tracker = AgentTracker::with_default_timeout();
        let now = Utc::now().timestamp();

        // Multiple activities on same file
        for i in 0..5 {
            let activity = AgentActivity::new(now + i, "agent-1", "edit", "src/lib.rs");
            tracker.update_from_activity(&activity).await;
        }

        assert_eq!(tracker.get_reference_count("src/lib.rs").await, 5);
    }

    #[tokio::test]
    async fn get_reference_count_tracks_multiple_files_independently() {
        let tracker = AgentTracker::with_default_timeout();
        let now = Utc::now().timestamp();

        // Activities on different files
        let activity1 = AgentActivity::new(now, "agent-1", "edit", "src/a.rs");
        tracker.update_from_activity(&activity1).await;

        let activity2 = AgentActivity::new(now + 1, "agent-2", "edit", "src/b.rs");
        tracker.update_from_activity(&activity2).await;

        let activity3 = AgentActivity::new(now + 2, "agent-1", "read", "src/a.rs");
        tracker.update_from_activity(&activity3).await;

        assert_eq!(tracker.get_reference_count("src/a.rs").await, 2);
        assert_eq!(tracker.get_reference_count("src/b.rs").await, 1);
    }

    #[tokio::test]
    async fn get_reference_count_survives_done_activity() {
        let tracker = AgentTracker::with_default_timeout();
        let now = Utc::now().timestamp();

        // Activity that marks file as done still counts as a reference
        let mut activity = AgentActivity::new(now, "agent-1", "edit", "src/foo.rs");
        activity.done = Some(true);
        tracker.update_from_activity(&activity).await;

        assert_eq!(tracker.get_reference_count("src/foo.rs").await, 1);
    }

    // ========== AgentTracker::prune_stale with file_references ==========

    #[tokio::test]
    async fn prune_stale_resets_counts_for_stale_agent_files() {
        let tracker = AgentTracker::new(Duration::seconds(60));
        let now = Utc::now().timestamp();

        // Fresh agent activity on file A
        let activity1 = AgentActivity::new(now, "fresh-agent", "edit", "src/a.rs");
        tracker.update_from_activity(&activity1).await;

        // Stale agent activity on file B
        let old_ts = (Utc::now() - Duration::seconds(61)).timestamp();
        let activity2 = AgentActivity::new(old_ts, "stale-agent", "edit", "src/b.rs");
        tracker.update_from_activity(&activity2).await;

        // Both files have counts
        assert_eq!(tracker.get_reference_count("src/a.rs").await, 1);
        assert_eq!(tracker.get_reference_count("src/b.rs").await, 1);

        // Prune stale agents
        tracker.prune_stale().await;

        // File B count should be reset (only stale agent referenced it)
        assert_eq!(tracker.get_reference_count("src/a.rs").await, 1);
        assert_eq!(tracker.get_reference_count("src/b.rs").await, 0);
    }

    #[tokio::test]
    async fn prune_stale_preserves_counts_for_shared_files() {
        let tracker = AgentTracker::new(Duration::seconds(60));
        let now = Utc::now().timestamp();

        // Fresh agent on file A
        let activity1 = AgentActivity::new(now, "fresh-agent", "edit", "src/shared.rs");
        tracker.update_from_activity(&activity1).await;

        // Stale agent also on file A
        let old_ts = (Utc::now() - Duration::seconds(61)).timestamp();
        let activity2 = AgentActivity::new(old_ts, "stale-agent", "read", "src/shared.rs");
        tracker.update_from_activity(&activity2).await;

        // Count is 2
        assert_eq!(tracker.get_reference_count("src/shared.rs").await, 2);

        // Prune stale agents
        tracker.prune_stale().await;

        // Count stays at 2 because fresh agent also references this file
        // (we can't decrement per-agent counts without per-agent tracking)
        assert_eq!(tracker.get_reference_count("src/shared.rs").await, 2);
    }
}
