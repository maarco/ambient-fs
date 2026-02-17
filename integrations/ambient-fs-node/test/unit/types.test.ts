/**
 * unit tests for type definitions
 * ensures types match rust schema and serialize correctly
 */

import {
  EventType,
  Source,
  ChangeFrequency,
  LintSeverity,
  type FileEvent,
  type FileAwareness,
  type TreeNode,
  type EventFilter,
  type ImportRef,
  type LintHint,
  type FileAnalysis,
} from "../../src/types.js";

describe("EventType", () => {
  it("should have all event types", () => {
    expect(EventType.Created).toBe("created");
    expect(EventType.Modified).toBe("modified");
    expect(EventType.Deleted).toBe("deleted");
    expect(EventType.Renamed).toBe("renamed");
  });

  it("should serialize to string", () => {
    const event: FileEvent = {
      timestamp: "2026-02-16T12:00:00Z",
      event_type: EventType.Created,
      file_path: "/test/file.txt",
      project_id: "proj-123",
      source: Source.User,
      machine_id: "machine-1",
    };

    const json = JSON.stringify(event);
    const parsed = JSON.parse(json) as FileEvent;

    expect(parsed.event_type).toBe("created");
  });
});

describe("Source", () => {
  it("should have all source types", () => {
    expect(Source.User).toBe("user");
    expect(Source.AiAgent).toBe("ai_agent");
    expect(Source.Git).toBe("git");
    expect(Source.Build).toBe("build");
    expect(Source.Voice).toBe("voice");
  });
});

describe("ChangeFrequency", () => {
  it("should have all frequency levels", () => {
    expect(ChangeFrequency.Hot).toBe("hot");
    expect(ChangeFrequency.Warm).toBe("warm");
    expect(ChangeFrequency.Cold).toBe("cold");
  });
});

describe("LintSeverity", () => {
  it("should have all severity levels", () => {
    expect(LintSeverity.Info).toBe("info");
    expect(LintSeverity.Warning).toBe("warning");
    expect(LintSeverity.Error).toBe("error");
  });
});

describe("FileEvent", () => {
  it("should accept minimal required fields", () => {
    const event: FileEvent = {
      timestamp: "2026-02-16T12:00:00Z",
      event_type: EventType.Modified,
      file_path: "/test/file.ts",
      project_id: "proj-123",
      source: Source.User,
      machine_id: "machine-1",
    };

    expect(event.file_path).toBe("/test/file.ts");
  });

  it("should accept optional fields", () => {
    const event: FileEvent = {
      timestamp: "2026-02-16T12:00:00Z",
      event_type: EventType.Renamed,
      file_path: "/test/new.ts",
      project_id: "proj-123",
      source: Source.Git,
      machine_id: "machine-1",
      source_id: "git-sha",
      content_hash: "abc123",
      old_path: "/test/old.ts",
    };

    expect(event.source_id).toBe("git-sha");
    expect(event.content_hash).toBe("abc123");
    expect(event.old_path).toBe("/test/old.ts");
  });

  it("should serialize and deserialize", () => {
    const original: FileEvent = {
      timestamp: "2026-02-16T12:00:00Z",
      event_type: EventType.Created,
      file_path: "/test/file.txt",
      project_id: "proj-123",
      source: Source.AiAgent,
      machine_id: "machine-1",
      source_id: "agent-42",
    };

    const json = JSON.stringify(original);
    const parsed = JSON.parse(json) as FileEvent;

    expect(parsed).toEqual(original);
  });
});

describe("FileAwareness", () => {
  it("should accept all fields", () => {
    const awareness: FileAwareness = {
      file_path: "/test/file.ts",
      project_id: "proj-123",
      last_modified: "2026-02-16T12:00:00Z",
      modified_by: Source.AiAgent,
      modified_by_label: "claude-opus",
      active_agent: "agent-42",
      chat_references: 3,
      todo_count: 5,
      lint_hints: 2,
      line_count: 150,
      change_frequency: ChangeFrequency.Hot,
    };

    expect(awareness.modified_by_label).toBe("claude-opus");
    expect(awareness.active_agent).toBe("agent-42");
    expect(awareness.change_frequency).toBe(ChangeFrequency.Hot);
  });

  it("should work without optional fields", () => {
    const awareness: FileAwareness = {
      file_path: "/test/file.ts",
      project_id: "proj-123",
      last_modified: "2026-02-16T12:00:00Z",
      modified_by: Source.User,
      chat_references: 0,
      todo_count: 0,
      lint_hints: 0,
      line_count: 50,
      change_frequency: ChangeFrequency.Cold,
    };

    expect(awareness.modified_by_label).toBeUndefined();
    expect(awareness.active_agent).toBeUndefined();
  });
});

describe("TreeNode", () => {
  it("should represent directory structure", () => {
    const tree: TreeNode = {
      name: "src",
      path: "/project/src",
      is_dir: true,
      children: [
        {
          name: "index.ts",
          path: "/project/src/index.ts",
          is_dir: false,
          children: [],
        },
        {
          name: "utils",
          path: "/project/src/utils",
          is_dir: true,
          children: [
            {
              name: "helper.ts",
              path: "/project/src/utils/helper.ts",
              is_dir: false,
              children: [],
            },
          ],
        },
      ],
    };

    expect(tree.children).toHaveLength(2);
    expect(tree.children[0].is_dir).toBe(false);
    expect(tree.children[1].is_dir).toBe(true);
  });
});

describe("EventFilter", () => {
  it("should accept empty filter", () => {
    const filter: EventFilter = {};
    expect(Object.keys(filter)).toHaveLength(0);
  });

  it("should accept partial filters", () => {
    const filter1: EventFilter = { project_id: "proj-123" };
    const filter2: EventFilter = { since: Date.now() - 3600000 };
    const filter3: EventFilter = { source: "ai_agent", limit: 50 };

    expect(filter1.project_id).toBe("proj-123");
    expect(filter2.since).toBeDefined();
    expect(filter3.limit).toBe(50);
  });
});

describe("ImportRef", () => {
  it("should represent import reference", () => {
    const importRef: ImportRef = {
      path: "./utils/helper",
      symbols: ["helperFunction", "HelperClass"],
      line: 5,
    };

    expect(importRef.symbols).toHaveLength(2);
    expect(importRef.line).toBe(5);
  });
});

describe("LintHint", () => {
  it("should represent lint hint", () => {
    const hint: LintHint = {
      line: 42,
      column: 10,
      severity: LintSeverity.Warning,
      message: "unused variable 'foo'",
      rule: "no-unused-vars",
    };

    expect(hint.severity).toBe(LintSeverity.Warning);
    expect(hint.rule).toBe("no-unused-vars");
  });

  it("should work without rule", () => {
    const hint: LintHint = {
      line: 10,
      column: 0,
      severity: LintSeverity.Error,
      message: "syntax error",
    };

    expect(hint.rule).toBeUndefined();
  });
});

describe("FileAnalysis", () => {
  it("should represent full file analysis", () => {
    const analysis: FileAnalysis = {
      file_path: "/test/file.ts",
      project_id: "proj-123",
      content_hash: "abc123def",
      exports: ["default", "helperFunction"],
      imports: [
        { path: "fs", symbols: ["readFileSync"], line: 1 },
        { path: "./types", symbols: ["MyType"], line: 2 },
      ],
      todo_count: 3,
      lint_hints: [
        {
          line: 15,
          column: 5,
          severity: LintSeverity.Warning,
          message: "TODO: refactor",
        },
      ],
      line_count: 120,
    };

    expect(analysis.exports).toHaveLength(2);
    expect(analysis.imports).toHaveLength(2);
    expect(analysis.todo_count).toBe(3);
  });
});
