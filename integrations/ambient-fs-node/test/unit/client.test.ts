/**
 * unit tests for ambient-fs client
 */

import { AmbientFsClient } from "../../src/client.js";
import { Transport } from "../../src/transport.js";
import { AmbientFsError } from "../../src/errors.js";
import type { FileEvent, FileAwareness, TreeNode } from "../../src/types.js";

// mock transport
class MockTransport {
  connected = true;
  notificationCallback: ((notification: unknown) => void) | null = null;

  async connect(): Promise<void> {
    // instant mock connection
  }

  close(): void {
    this.connected = false;
  }

  setNotificationCallback(cb: (notification: unknown) => void): void {
    this.notificationCallback = cb;
  }

  async request<T = unknown>(method: string, params?: Record<string, unknown>): Promise<T> {
    // mock responses based on method
    switch (method) {
      case "watch_project":
        return { project_id: "proj-123", watching: true } as T;
      case "unwatch_project":
        return undefined as T;
      case "query_events":
        return [
          {
            timestamp: "2026-02-16T12:00:00Z",
            event_type: "Modified",
            file_path: "/test/file.ts",
            project_id: params?.project_id,
            source: "User",
            machine_id: "machine-1",
          },
        ] as T;
      case "query_awareness":
        return {
          file_path: params?.path,
          project_id: params?.project_id,
          last_modified: "2026-02-16T12:00:00Z",
          modified_by: "User",
          chat_references: 0,
          todo_count: 5,
          lint_hints: 2,
          line_count: 100,
          change_frequency: "Warm",
        } as T;
      case "query_tree":
        return {
          name: "root",
          path: "/test",
          is_dir: true,
          children: [
            { name: "file.ts", path: "/test/file.ts", is_dir: false, children: [] },
          ],
        } as T;
      case "attribute":
        return undefined as T;
      case "subscribe":
        return undefined as T;
      case "unsubscribe":
        return undefined as T;
      default:
        throw new AmbientFsError(`unknown method: ${method}`, "DAEMON_ERROR");
    }
  }

  isConnected(): boolean {
    return this.connected;
  }
}

describe("AmbientFsClient", () => {
  describe("connect", () => {
    it("should connect with default socket path", async () => {
      const client = await AmbientFsClient.connect();
      expect(client.isConnected()).toBe(true);
      client.close();
    });

    it("should connect with custom socket path", async () => {
      const client = await AmbientFsClient.connect({ socketPath: "/custom/path.sock" });
      expect(client.isConnected()).toBe(true);
      client.close();
    });

    it("should use AMBIENT_FS_SOCK env var", async () => {
      const originalEnv = process.env.AMBIENT_FS_SOCK;
      process.env.AMBIENT_FS_SOCK = "/env/path.sock";

      const client = await AmbientFsClient.connect();
      expect(client.isConnected()).toBe(true);

      process.env.AMBIENT_FS_SOCK = originalEnv;
      client.close();
    });
  });

  describe("watchProject", () => {
    it("should watch a project directory", async () => {
      const client = await AmbientFsClient.connect();
      const result = await client.watchProject("/home/user/project");

      expect(result).toEqual({
        project_id: "proj-123",
        watching: true,
      });
      client.close();
    });
  });

  describe("unwatchProject", () => {
    it("should unwatch a project", async () => {
      const client = await AmbientFsClient.connect();
      await expect(client.unwatchProject("proj-123")).resolves.toBeUndefined();
      client.close();
    });
  });

  describe("queryEvents", () => {
    it("should query events with filter", async () => {
      const client = await AmbientFsClient.connect();
      const events = await client.queryEvents({
        project_id: "proj-123",
        since: Date.now() - 3600000,
        limit: 100,
      });

      expect(events).toHaveLength(1);
      expect(events[0].project_id).toBe("proj-123");
      client.close();
    });

    it("should query all events when no filter", async () => {
      const client = await AmbientFsClient.connect();
      const events = await client.queryEvents();

      expect(Array.isArray(events)).toBe(true);
      client.close();
    });
  });

  describe("queryAwareness", () => {
    it("should query file awareness", async () => {
      const client = await AmbientFsClient.connect();
      const awareness = await client.queryAwareness("proj-123", "src/main.ts");

      expect(awareness.file_path).toBe("src/main.ts");
      expect(awareness.project_id).toBe("proj-123");
      expect(awareness.todo_count).toBe(5);
      expect(awareness.change_frequency).toBe("Warm");
      client.close();
    });
  });

  describe("queryTree", () => {
    it("should query project tree", async () => {
      const client = await AmbientFsClient.connect();
      const tree = await client.queryTree("proj-123");

      expect(tree.name).toBe("root");
      expect(tree.is_dir).toBe(true);
      expect(tree.children).toHaveLength(1);
      expect(tree.children[0].name).toBe("file.ts");
      client.close();
    });
  });

  describe("attribute", () => {
    it("should attribute file to source", async () => {
      const client = await AmbientFsClient.connect();

      await expect(
        client.attribute("proj-123", "src/file.ts", "ai_agent", "agent-1")
      ).resolves.toBeUndefined();

      client.close();
    });

    it("should work without source_id", async () => {
      const client = await AmbientFsClient.connect();

      await expect(
        client.attribute("proj-123", "src/file.ts", "git")
      ).resolves.toBeUndefined();

      client.close();
    });
  });

  describe("subscribe", () => {
    it("should subscribe to notifications", async () => {
      const client = await AmbientFsClient.connect();

      const notifications: unknown[] = [];
      const unsubscribe = await client.subscribe("proj-123", (notif) => {
        notifications.push(notif);
      });

      expect(typeof unsubscribe).toBe("function");
      client.close();
    });

    it("should receive file_event notifications", async () => {
      const client = await AmbientFsClient.connect();

      const notifications: unknown[] = [];
      await client.subscribe("proj-123", (notif) => {
        notifications.push(notif);
      });

      // simulate notification from transport
      const transport = (client as any).transport as MockTransport;
      transport.notificationCallback?.({
        jsonrpc: "2.0",
        method: "file_event",
        params: {
          project_id: "proj-123",
          event: { file_path: "test.ts", type: "Modified" },
        },
      });

      expect(notifications).toHaveLength(1);
      expect(notifications[0]).toEqual({
        type: "file_event",
        projectId: "proj-123",
        data: {
          project_id: "proj-123",
          event: { file_path: "test.ts", type: "Modified" },
        },
      });

      client.close();
    });

    it("should filter notifications by project_id", async () => {
      const client = await AmbientFsClient.connect();

      const notifications: unknown[] = [];
      await client.subscribe("proj-123", (notif) => {
        notifications.push(notif);
      });

      const transport = (client as any).transport as MockTransport;

      // send notification for different project
      transport.notificationCallback?.({
        jsonrpc: "2.0",
        method: "file_event",
        params: {
          project_id: "other-proj",
          event: { file_path: "other.ts", type: "Created" },
        },
      });

      expect(notifications).toHaveLength(0);

      client.close();
    });

    it("should handle awareness_changed notifications", async () => {
      const client = await AmbientFsClient.connect();

      const notifications: unknown[] = [];
      await client.subscribe("proj-123", (notif) => {
        notifications.push(notif);
      });

      const transport = (client as any).transport as MockTransport;
      transport.notificationCallback?.({
        jsonrpc: "2.0",
        method: "awareness_changed",
        params: {
          project_id: "proj-123",
          awareness: { file_path: "test.ts", change_frequency: "Hot" },
        },
      });

      expect(notifications).toHaveLength(1);
      expect(notifications[0].type).toBe("awareness_changed");

      client.close();
    });

    it("should handle analysis_complete notifications", async () => {
      const client = await AmbientFsClient.connect();

      const notifications: unknown[] = [];
      await client.subscribe("proj-123", (notif) => {
        notifications.push(notif);
      });

      const transport = (client as any).transport as MockTransport;
      transport.notificationCallback?.({
        jsonrpc: "2.0",
        method: "analysis_complete",
        params: {
          project_id: "proj-123",
          analysis: { file_path: "test.ts", exports: [] },
        },
      });

      expect(notifications).toHaveLength(1);
      expect(notifications[0].type).toBe("analysis_complete");

      client.close();
    });
  });

  describe("unsubscribe", () => {
    it("should unsubscribe from notifications", async () => {
      const client = await AmbientFsClient.connect();

      await expect(client.unsubscribe("proj-123")).resolves.toBeUndefined();
      client.close();
    });
  });

  describe("close", () => {
    it("should close connection and clear state", async () => {
      const client = await AmbientFsClient.connect();

      expect(client.isConnected()).toBe(true);
      client.close();
      expect(client.isConnected()).toBe(false);
    });
  });
});
