// TypeScript API for ambient-fs tauri plugin
// usage: import * as AmbientFs from '@ambient-fs/tauri-plugin'

import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import type {
    FileEvent,
    FileAwareness,
    TreeNode,
    EventFilter,
    AgentInfo,
    AwarenessChangedEvent,
    AnalysisCompleteEvent,
    FileAnalysis,
    DaemonStatus,
} from './types';

// ===== IPC command wrappers =====

export async function watchProject(path: string): Promise<string> {
    return invoke('plugin:ambient-fs|watch_project', { path });
}

export async function unwatchProject(projectId: string): Promise<void> {
    return invoke('plugin:ambient-fs|unwatch_project', { projectId });
}

export async function queryEvents(filter: EventFilter): Promise<FileEvent[]> {
    return invoke('plugin:ambient-fs|query_events', { filter });
}

export async function queryAwareness(projectId: string, filePath: string): Promise<FileAwareness> {
    return invoke('plugin:ambient-fs|query_awareness', { projectId, filePath });
}

export async function queryTree(projectId: string): Promise<TreeNode> {
    return invoke('plugin:ambient-fs|query_tree', { projectId });
}

export async function attribute(
    projectId: string,
    filePath: string,
    source: string,
    sourceId?: string,
): Promise<void> {
    return invoke('plugin:ambient-fs|attribute', { projectId, filePath, source, sourceId });
}

export async function queryAgents(): Promise<AgentInfo[]> {
    return invoke('plugin:ambient-fs|query_agents');
}

export async function getStatus(): Promise<DaemonStatus> {
    return invoke('plugin:ambient-fs|get_status');
}

// ===== Event listeners =====

export function onFileEvent(callback: (event: FileEvent) => void): UnlistenFn {
    return listen<FileEvent>('ambient-fs://event', (e) => callback(e.payload));
}

export function onAwarenessChanged(callback: (data: AwarenessChangedEvent) => void): UnlistenFn {
    return listen('ambient-fs://awareness-changed', (e) => callback(e.payload));
}

export function onAnalysisComplete(callback: (data: AnalysisCompleteEvent) => void): UnlistenFn {
    return listen('ambient-fs://analysis-complete', (e) => callback(e.payload));
}

export function onConnectedChanged(callback: (connected: boolean) => void): UnlistenFn {
    return listen<{connected: boolean}>('ambient-fs://connected', (e) => callback(e.payload.connected));
}

// ===== Re-export types =====

export type {
    FileEvent,
    FileAwareness,
    TreeNode,
    EventFilter,
    AgentInfo,
    AwarenessChangedEvent,
    AnalysisCompleteEvent,
    FileAnalysis,
    DaemonStatus,
};
