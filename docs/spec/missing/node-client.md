node client implementation
=========================

status: design
created: 2026-02-16
affects: integrations/ambient-fs-node/


overview
--------

idiomatic node.js/typescript client for ambient-fs daemon.
unix socket json-rpc 2.0, minimal dependencies, full api
coverage matching the rust client.

target package: @ambient-fs/client
repo location: integrations/ambient-fs-node/


package structure
-----------------

integrations/ambient-fs-node/
  package.json
  tsconfig.json
  src/
    index.ts           (main export)
    client.ts          (AmbientFsClient class)
    types.ts           (ts types matching ambient-fs-core)
    transport.ts       (unix socket + json-rpc framing)
    errors.ts          (AmbientFsError class)
  test/
    unit/              (mock socket tests)
    integration/       (real daemon tests)


package.json
------------

```json
{
  "name": "@ambient-fs/client",
  "version": "0.1.0",
  "description": "Node.js client for ambient-fs filesystem awareness daemon",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "files": ["dist"],
  "scripts": {
    "build": "tsc",
    "test": "jest",
    "test:unit": "jest test/unit",
    "test:integration": "jest test/integration",
    "prepublishOnly": "npm run build"
  },
  "keywords": ["filesystem", "watcher", "daemon", "ipc"],
  "license": "MIT",
  "devDependencies": {
    "@types/node": "^20",
    "typescript": "^5",
    "jest": "^29",
    "@types/jest": "^29",
    "ts-jest": "^29"
  },
  "engines": {
    "node": ">=18"
  },
  "peerDependencies": {},
  "dependencies": {}
}
```

notes: no runtime deps. node built-in net module handles unix sockets.


tsconfig.json
-------------

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "lib": ["ES2022"],
    "outDir": "dist",
    "rootDir": "src",
    "declaration": true,
    "declarationMap": true,
    "sourceMap": true,
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true
  },
  "include": ["src/**/*"],
  "exclude": ["node_modules", "dist", "test"]
}
```


types.ts
--------

typescript definitions matching ambient-fs-core enums/structs.
all serde-compatible with the rust types.

```typescript
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
```


errors.ts
---------

```typescript
export class AmbientFsError extends Error {
  constructor(
    message: string,
    public code: ErrorCode,
    public details?: unknown
  ) {
    super(message);
    this.name = "AmbientFsError";
  }

  static notConnected(): AmbientFsError {
    return new AmbientFsError("daemon not connected", "NOT_CONNECTED");
  }

  static timeout(): AmbientFsError {
    return new AmbientFsError("operation timed out", "TIMEOUT");
  }

  static daemonError(message: string, code?: number): AmbientFsError {
    return new AmbientFsError(
      `daemon returned error: ${message}`,
      "DAEMON_ERROR",
      { daemonCode: code }
    );
  }

  static invalidResponse(message: string): AmbientFsError {
    return new AmbientFsError(`invalid response from daemon: ${message}`, "INVALID_RESPONSE");
  }

  static parseError(message: string): AmbientFsError {
    return new AmbientFsError(`json parse error: ${message}`, "PARSE_ERROR");
  }
}

export type ErrorCode =
  | "NOT_CONNECTED"
  | "TIMEOUT"
  | "DAEMON_ERROR"
  | "INVALID_RESPONSE"
  | "PARSE_ERROR";
```


transport.ts
-----------

unix socket wrapper + json-rpc framing.

```typescript
import { Socket } from "net";
import { AmbientFsError } from "./errors.js";

export interface JsonRpcRequest {
  jsonrpc: "2.0";
  method: string;
  params?: Record<string, unknown> | unknown[];
  id: number;
}

export interface JsonRpcResponse {
  jsonrpc: "2.0";
  result?: unknown;
  error?: {
    code: number;
    message: string;
    data?: unknown;
  };
  id: number;
}

export interface JsonRpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: Record<string, unknown> | unknown[];
}

export class Transport {
  private socket: Socket | null = null;
  private requestId = 1;
  private pending = new Map<number, {
    resolve: (value: unknown) => void;
    reject: (error: Error) => void;
    timeout: NodeJS.Timeout;
  }>();
  private notificationCallback: ((notification: JsonRpcNotification) => void) | null = null;
  private buffer = "";

  constructor(private socketPath: string) {}

  connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.socket = new Socket();

      const timeout = setTimeout(() => {
        this.socket?.destroy();
        reject(AmbientFsError.timeout());
      }, 5000);

      this.socket.once("connect", () => {
        clearTimeout(timeout);
        resolve();
      });

      this.socket.once("error", (err) => {
        clearTimeout(timeout);
        reject(new AmbientFsError(`connect failed: ${err.message}`, "NOT_CONNECTED"));
      });

      this.socket.on("data", (data: Buffer) => this.handleData(data));
      this.socket.connect(this.socketPath);
    });
  }

  close(): void {
    // clear all pending
    for (const { timeout, reject } of this.pending.values()) {
      clearTimeout(timeout);
      reject(AmbientFsError.notConnected());
    }
    this.pending.clear();
    this.socket?.destroy();
    this.socket = null;
  }

  setNotificationCallback(cb: (notification: JsonRpcNotification) => void): void {
    this.notificationCallback = cb;
  }

  async request<T = unknown>(method: string, params?: Record<string, unknown>): Promise<T> {
    if (!this.socket) {
      throw AmbientFsError.notConnected();
    }

    const id = this.requestId++;
    const request: JsonRpcRequest = { jsonrpc: "2.0", method, params, id };

    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        reject(AmbientFsError.timeout());
      }, 30000); // 30s default

      this.pending.set(id, { resolve, reject, timeout });

      const json = JSON.stringify(request) + "\n";
      this.socket!.write(json, (err) => {
        if (err) {
          clearTimeout(timeout);
          this.pending.delete(id);
          reject(new AmbientFsError(`write failed: ${err.message}`, "NOT_CONNECTED"));
        }
      });
    });
  }

  private handleData(data: Buffer): void {
    this.buffer += data.toString("utf-8");
    const lines = this.buffer.split("\n");
    this.buffer = lines.pop() || ""; // keep partial

    for (const line of lines) {
      if (!line.trim()) continue;
      try {
        const msg = JSON.parse(line);
        if ("id" in msg && typeof msg.id === "number") {
          this.handleResponse(msg as JsonRpcResponse);
        } else if ("method" in msg) {
          this.notificationCallback?.(msg as JsonRpcNotification);
        }
      } catch (e) {
        // log but don't crash
        console.error("failed to parse json-rpc message:", e);
      }
    }
  }

  private handleResponse(response: JsonRpcResponse): void {
    const pending = this.pending.get(response.id);
    if (!pending) return;

    this.pending.delete(response.id);
    clearTimeout(pending.timeout);

    if (response.error) {
      pending.reject(AmbientFsError.daemonError(response.error.message, response.error.code));
    } else {
      pending.resolve(response.result);
    }
  }

  isConnected(): boolean {
    return this.socket?.readyState === "open";
  }
}
```


client.ts
--------

main client class. static connect() method + protocol methods.

```typescript
import { Transport } from "./transport.js";
import { AmbientFsError } from "./errors.js";
import type {
  FileEvent, FileAwareness, TreeNode, EventFilter,
  JsonRpcRequest, JsonRpcResponse,
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
```


index.ts
-------

public api exports.

```typescript
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
```


usage examples
--------------

basic connect + watch:

```typescript
import { AmbientFsClient } from "@ambient-fs/client";

const client = await AmbientFsClient.connect();
const { project_id } = await client.watchProject("/home/user/myproject");

// query events
const events = await client.queryEvents({
  project_id,
  since: Date.now() - 3600000, // last hour
});

console.log(`found ${events.length} events`);

client.close();
```

subscribing to notifications:

```typescript
const client = await AmbientFsClient.connect();
await client.watchProject("/home/user/myproject");

const unsubscribe = await client.subscribe("project-id", (notification) => {
  if (notification.type === "file_event") {
    console.log("file changed:", notification.data);
  }
});

// later
unsubscribe();
client.close();
```


querying awareness:

```typescript
const client = await AmbientFsClient.connect();

const awareness = await client.queryAwareness("project-id", "src/main.ts");
console.log(`last modified ${awareness.modified_by}`);
console.log(`change frequency: ${awareness.change_frequency}`);
console.log(`todos: ${awareness.todo_count}`);

client.close();
```


custom socket path:

```typescript
const client = await AmbientFsClient.connect({
  socketPath: "/custom/path/ambient-fs.sock",
});

// or via env
process.env.AMBIENT_FS_SOCK = "/custom/path/ambient-fs.sock";
const client = await AmbientFsClient.connect();
```


error handling:

```typescript
import { AmbientFsError } from "@ambient-fs/client";

try {
  const client = await AmbientFsClient.connect();
  // ...
} catch (e) {
  if (e instanceof AmbientFsError) {
    if (e.code === "NOT_CONNECTED") {
      console.error("daemon not running");
    } else if (e.code === "TIMEOUT") {
      console.error("operation timed out");
    }
  }
}
```


testing strategy
----------------

unit tests (test/unit/):
  - mock socket using duplex stream
  - verify json-rpc request format
  - verify response parsing
  - error propagation
  - filter serialization

integration tests (test/integration/):
  - require running daemon
  - start real server in fixture
  - full roundtrip: connect -> watch -> query -> close
  - subscribe + receive notification
  - error cases: invalid path, unknown project

jest config:

```javascript
module.exports = {
  preset: "ts-jest",
  testEnvironment: "node",
  roots: ["<rootDir>/test"],
  testMatch: ["**/*.test.ts"],
  collectCoverageFrom: ["src/**/*.ts"],
};
```


socket path resolution order
-----------------------------

1. explicit config.socketPath
2. process.env.AMBIENT_FS_SOCK
3. "/tmp/ambient-fs.sock"


build + publish
---------------

npm run build     # tsc -> dist/
npm test
npm publish

package.json "files" includes only dist/, keeping source and test private.


implementation checklist
------------------------

  ☐ package.json + tsconfig.json
  ☐ types.ts (all enums/interfaces)
  ☐ errors.ts (AmbientFsError)
  ☐ transport.ts (unix socket + json-rpc framing)
  ☐ client.ts (main api)
  ☐ index.ts (exports)
  ☐ unit tests (mock socket)
  ☐ integration tests (real daemon)
  ☐ build + verify dist/
  ☐ readme with usage examples
