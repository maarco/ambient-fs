# Node Client Gaps - Agent Methods Implementation

## Overview

The Node TypeScript client is missing agent-related methods that are implemented on the server.
This spec defines the missing functionality for agent tracking and activity reporting.

## Server API Reference

From `crates/ambient-fs-server/src/socket.rs`:

| Method | Params | Returns |
|--------|--------|---------|
| `query_agents` | `{file?: string}` | `AgentInfo[]` |
| `watch_agents` | `{path: string}` | `{watching: true, path: string}` |
| `unwatch_agents` | `{path: string}` | `true` |
| `report_agent_activity` | `AgentActivity` | `{recorded: true}` |

## Missing Types

```typescript
// Agent information returned by query_agents
export interface AgentInfo {
  agent_id: string;        // e.g. "claude-code", "cursor"
  files: string[];         // Absolute paths agent is working on
  last_seen: number;       // Unix timestamp (seconds)
  intent?: string;         // Optional description of what agent is doing
  tool?: string;           // Tool being used (e.g. "Edit", "Bash")
  session?: string;        // Session identifier
}

// Agent activity for reporting
export interface AgentActivity {
  ts: number;              // Unix timestamp (seconds)
  agent: string;           // Agent name
  action: string;          // Action type (e.g. "edit", "read", "run")
  file: string;            // Absolute file path
  project?: string;        // Optional project_id
  tool?: string;           // Tool being used
  session?: string;        // Session identifier
  intent?: string;         // What agent is trying to do
  lines?: number[];        // Line numbers being edited
  confidence?: number;     // 0.0-1.0 confidence level
  done?: boolean;          // Is action complete
}
```

## Missing Client Methods

### 1. queryAgents

```typescript
/**
 * Query active agents, optionally filtered by file.
 *
 * @param filter - Optional filter object
 * @param filter.file - Only return agents working on this file
 * @returns Array of agent information
 */
async queryAgents(filter?: { file?: string }): Promise<AgentInfo[]>
```

Example:
```typescript
// Get all active agents
const agents = await client.queryAgents();

// Get agents working on specific file
const fileAgents = await client.queryAgents({ file: "/abs/path/to/file.rs" });
```

### 2. watchAgents

```typescript
/**
 * Register a directory to watch for agent activity JSONL files.
 *
 * The daemon will tail JSONL files in this directory to track agent activity.
 *
 * @param path - Absolute path to agents directory (e.g. "/project/.agents")
 */
async watchAgents(path: string): Promise<{ watching: boolean; path: string }>
```

Example:
```typescript
await client.watchAgents("/Users/dev/myproject/.agents");
```

### 3. unwatchAgents

```typescript
/**
 * Stop watching a directory for agent activity.
 *
 * @param path - Absolute path to agents directory
 */
async unwatchAgents(path: string): Promise<void>
```

Example:
```typescript
await client.unwatchAgents("/Users/dev/myproject/.agents");
```

### 4. reportAgentActivity

```typescript
/**
 * Report agent activity directly to the daemon.
 *
 * This enables source attribution - files modified while an agent is active
 * will get source = AiAgent instead of User.
 *
 * @param activity - Agent activity record
 */
async reportAgentActivity(activity: AgentActivity): Promise<{ recorded: boolean }>
```

Example:
```typescript
await client.reportAgentActivity({
  ts: Math.floor(Date.now() / 1000),
  agent: "claude-code",
  action: "edit",
  file: "/Users/dev/project/src/main.rs",
  project: "myproject",
  tool: "Edit",
  session: "sess-123",
  intent: "fix auth bug",
  lines: [42, 67],
  done: true,
});
```

## Implementation Steps

### Step 1: Add types to types.ts

Add `AgentInfo` and `AgentActivity` interfaces to `src/types.ts`.

### Step 2: Add methods to client.ts

Add the four methods to `AmbientFsClient` class in `src/client.ts`:

```typescript
// ===== agent tracking =====

async queryAgents(filter?: { file?: string }): Promise<AgentInfo[]> {
  const params = filter || {};
  return this.transport.request<AgentInfo[]>("query_agents", params);
}

async watchAgents(path: string): Promise<{ watching: boolean; path: string }> {
  return this.transport.request<{ watching: boolean; path: string }>("watch_agents", { path });
}

async unwatchAgents(path: string): Promise<void> {
  await this.transport.request("unwatch_agents", { path });
}

async reportAgentActivity(activity: AgentActivity): Promise<{ recorded: boolean }> {
  return this.transport.request<{ recorded: boolean }>("report_agent_activity", activity);
}
```

### Step 3: Export types from index.ts

Add to type exports in `src/index.ts`:

```typescript
export type {
  // ... existing exports
  AgentInfo,
  AgentActivity,
} from "./types.js";
```

### Step 4: Tests

Create `test/agents.test.ts` with:

```typescript
import { describe, it, expect, beforeAll, afterAll } from "bun:test";
import { AmbientFsClient } from "../src/client";
import { AgentInfo, AgentActivity } from "../src/types";

describe("Agent Methods", () => {
  let client: AmbientFsClient;

  beforeAll(async () => {
    client = await AmbientFsClient.connect();
  });

  afterAll(() => {
    client.close();
  });

  describe("queryAgents", () => {
    it("should return empty array when no agents", async () => {
      const agents = await client.queryAgents();
      expect(Array.isArray(agents)).toBe(true);
    });

    it("should filter by file when provided", async () => {
      const agents = await client.queryAgents({ file: "/some/path.rs" });
      expect(Array.isArray(agents)).toBe(true);
    });
  });

  describe("watchAgents / unwatchAgents", () => {
    it("should watch agents directory", async () => {
      const result = await client.watchAgents("/tmp/test-agents");
      expect(result.watching).toBe(true);
      expect(result.path).toBe("/tmp/test-agents");

      await client.unwatchAgents("/tmp/test-agents");
    });

    it("should unwatch agents directory", async () => {
      await client.watchAgents("/tmp/test-agents2");
      await client.unwatchAgents("/tmp/test-agents2");
      // no error thrown
    });
  });

  describe("reportAgentActivity", () => {
    it("should record agent activity", async () => {
      const activity: AgentActivity = {
        ts: Math.floor(Date.now() / 1000),
        agent: "test-agent",
        action: "edit",
        file: "/tmp/test.ts",
        done: true,
      };

      const result = await client.reportAgentActivity(activity);
      expect(result.recorded).toBe(true);
    });
  });
});
```

## Notes

- All methods use JSON-RPC via `transport.request()`
- Server handler paths: `handle_query_agents`, `handle_watch_agents`, `handle_unwatch_agents`, `handle_report_agent_activity`
- Agent activity enables source attribution for file events
- `watchAgents` sets up passive JSONL file watching (server-side)
- `reportAgentActivity` is for direct reporting (e.g. from active agent)
