export { AmbientFsClient, type ClientConfig, type NotificationCallback } from "./client.js";
export { AmbientFsError, type ErrorCode } from "./errors.js";
export type {
  FileEvent,
  EventType,
  Source,
  FileAwareness,
  ChangeFrequency,
  TreeNode,
  EventFilter,
  ImportRef,
  LintHint,
  LintSeverity,
  FileAnalysis,
} from "./types.js";

export const DEFAULT_SOCKET_PATH = "/tmp/ambient-fs.sock";
