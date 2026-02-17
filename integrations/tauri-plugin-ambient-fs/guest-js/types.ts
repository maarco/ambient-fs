// type definitions for ambient-fs tauri plugin
// mirrors Rust types from ambient-fs-core

export type EventType = 'created' | 'modified' | 'deleted' | 'renamed';
export type Source = 'user' | 'ai_agent' | 'git' | 'build' | 'voice';
export type ChangeFrequency = 'hot' | 'warm' | 'cold';

export interface FileEvent {
    timestamp: string;           // ISO 8601
    event_type: EventType;
    file_path: string;
    project_id: string;
    source: Source;
    source_id?: string;
    machine_id: string;
    content_hash?: string;
    old_path?: string;
}

export interface FileAwareness {
    file_path: string;
    project_id: string;
    last_modified: string;        // ISO 8601
    modified_by: Source;
    modified_by_label?: string;
    active_agent?: string;
    chat_references: number;
    todo_count: number;
    lint_hints: number;
    line_count: number;
    change_frequency: ChangeFrequency;
}

export interface TreeNode {
    name: string;
    path: string;
    is_dir: boolean;
    children: TreeNode[];
}

export interface EventFilter {
    project_id?: string;
    since?: number;              // unix timestamp
    source?: string;
    limit?: number;
}

export interface AgentInfo {
    id: string;
    name: string;
    status: 'active' | 'idle' | 'error';
    project_id?: string;
    file_path?: string;
}

export interface AwarenessChangedEvent {
    project_id: string;
    file_path: string;
    awareness: FileAwareness;
}

export interface AnalysisCompleteEvent {
    project_id: string;
    file_path: string;
    analysis: FileAnalysis;
}

export interface FileAnalysis {
    imports: ImportRef[];
    exports: ExportRef[];
    todos: TodoHint[];
    lint: LintHint[];
}

export interface ImportRef {
    module: string;
    line: number;
}

export interface ExportRef {
    name: string;
    line: number;
}

export interface TodoHint {
    line: number;
    text: string;
    tag: 'TODO' | 'FIXME' | 'HACK' | 'NOTE';
}

export interface LintHint {
    line: number;
    message: string;
    severity: 'error' | 'warning' | 'info';
}

export interface DaemonStatus {
    connected: boolean;
}
