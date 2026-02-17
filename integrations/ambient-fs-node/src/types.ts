// event types
export enum EventType {
  Created = "created",
  Modified = "modified",
  Deleted = "deleted",
  Renamed = "renamed",
}

export enum Source {
  User = "user",
  AiAgent = "ai_agent",
  Git = "git",
  Build = "build",
  Voice = "voice",
}

export interface FileEvent {
  timestamp: string;           // ISO 8601 datetime
  event_type: EventType;
  file_path: string;
  project_id: string;
  source: Source;
  source_id?: string;
  machine_id: string;
  content_hash?: string;
  old_path?: string;           // for renames
}

// awareness
export enum ChangeFrequency {
  Hot = "hot",
  Warm = "warm",
  Cold = "cold",
}

export interface FileAwareness {
  file_path: string;
  project_id: string;
  last_modified: string;
  modified_by: Source;
  modified_by_label?: string;
  active_agent?: string;
  chat_references: number;
  todo_count: number;
  lint_hints: number;
  line_count: number;
  change_frequency: ChangeFrequency;
}

// tree
export interface TreeNode {
  name: string;
  path: string;
  is_dir: boolean;
  children: TreeNode[];
}

// analysis (if we expose query_analysis)
export interface ImportRef {
  path: string;
  symbols: string[];
  line: number;
}

export enum LintSeverity {
  Info = "info",
  Warning = "warning",
  Error = "error",
}

export interface LintHint {
  line: number;
  column: number;
  severity: LintSeverity;
  message: string;
  rule?: string;
}

export interface FileAnalysis {
  file_path: string;
  project_id: string;
  content_hash: string;
  exports: string[];
  imports: ImportRef[];
  todo_count: number;
  lint_hints: LintHint[];
  line_count: number;
}

// query filter
export interface EventFilter {
  project_id?: string;
  since?: number;              // unix timestamp ms
  source?: string;
  limit?: number;
}
