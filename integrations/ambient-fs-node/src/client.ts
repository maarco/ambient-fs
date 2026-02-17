import { Transport } from "./transport.js";
import { AmbientFsError } from "./errors.js";
import type {
  FileEvent, FileAwareness, TreeNode, EventFilter,
} from "./types.js";

export interface ClientConfig {
  socketPath?: string;
  timeout?: number;
}

export type NotificationCallback = (notification: {
  type: "file_event" | "awareness_changed" | "analysis_complete";
  projectId: string;
  data: unknown;
}) => void;

export class AmbientFsClient {
  private transport: Transport;
  private unsubscribeFns = new Map<string, () => void>();

  private constructor(socketPath: string) {
    this.transport = new Transport(socketPath);
  }

  static async connect(config?: ClientConfig): Promise<AmbientFsClient> {
    const socketPath = config?.socketPath || process.env.AMBIENT_FS_SOCK || "/tmp/ambient-fs.sock";
    const client = new AmbientFsClient(socketPath);
    await client.transport.connect();
    return client;
  }

  // ===== project management =====

  async watchProject(path: string): Promise<{ project_id: string; watching: boolean }> {
    return this.transport.request<{ project_id: string; watching: boolean }>("watch_project", { path });
  }

  async unwatchProject(projectId: string): Promise<void> {
    await this.transport.request("unwatch_project", { project_id: projectId });
  }

  // ===== queries =====

  async queryEvents(filter?: EventFilter): Promise<FileEvent[]> {
    return this.transport.request<FileEvent[]>("query_events", filter || {});
  }

  async queryAwareness(projectId: string, path: string): Promise<FileAwareness> {
    return this.transport.request<FileAwareness>("query_awareness", { project_id: projectId, path });
  }

  async queryTree(projectId: string): Promise<TreeNode> {
    return this.transport.request<TreeNode>("query_tree", { project_id: projectId });
  }

  // ===== attribution =====

  async attribute(
    projectId: string,
    filePath: string,
    source: string,
    sourceId?: string
  ): Promise<void> {
    const params: Record<string, string> = {
      project_id: projectId,
      file_path: filePath,
      source,
    };
    if (sourceId) params.source_id = sourceId;
    await this.transport.request("attribute", params);
  }

  // ===== subscriptions =====

  async subscribe(projectId: string, callback: NotificationCallback): Promise<() => void> {
    await this.transport.request("subscribe", { project_id: projectId });

    // set up notification handler for this project
    const handler = (notification: unknown) => {
      // notification format from server:
      // { method: "file_event", params: { project_id, event } }
      // or { method: "awareness_changed", params: { project_id, awareness } }
      const notif = notification as { method: string; params: { project_id?: string } };
      if (notif.params?.project_id === projectId) {
        const type = notif.method === "file_event" ? "file_event"
          : notif.method === "awareness_changed" ? "awareness_changed"
          : notif.method === "analysis_complete" ? "analysis_complete"
          : "file_event";
        callback({
          type,
          projectId,
          data: notif.params,
        });
      }
    };

    // store cleanup function
    const unsubscribe = () => {
      this.unsubscribe(projectId).catch(() => {});
    };
    this.unsubscribeFns.set(`${projectId}-${Date.now()}`, unsubscribe);

    return unsubscribe;
  }

  async unsubscribe(projectId: string): Promise<void> {
    await this.transport.request("unsubscribe", { project_id: projectId });
  }

  // ===== lifecycle =====

  close(): void {
    this.transport.close();
    this.unsubscribeFns.clear();
  }

  isConnected(): boolean {
    return this.transport.isConnected();
  }
}
