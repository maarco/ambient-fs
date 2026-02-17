agent activity protocol
========================

status: design
created: 2026-02-16
replaces: active-agent-protocol.md (claim/release approach)


overview
--------

instead of requiring agents to call a claim/release API,
the daemon passively watches a registered directory for
JSONL files that agents write during their work. any AI
coding tool (claude code, codex, opencode, cursor, etc)
can emit activity lines in a simple schema. the daemon
parses these to determine which agents are active on
which files.

a haiku-class LLM is optionally invoked to enhance
ambiguous or non-conforming output.


the schema (agent-activity-line)
--------------------------------

one JSON object per line. minimal required fields,
everything else optional.

required fields:

  {
    "ts": 1708099200,
    "agent": "claude-7",
    "action": "edit",
    "file": "src/main.rs"
  }

  ts       unix timestamp (seconds)
  agent    agent identifier (string, agent picks its own)
  action   what it's doing (see action vocab below)
  file     relative file path being acted on

optional fields:

  {
    "ts": 1708099200,
    "agent": "claude-7",
    "action": "edit",
    "file": "src/main.rs",
    "project": "ambient-fs",
    "tool": "claude-code",
    "session": "abc123",
    "intent": "fixing auth bypass in login handler",
    "lines": [42, 67],
    "confidence": 0.95,
    "done": false
  }

  project      project identifier
  tool         which AI tool (claude-code, codex, opencode, cursor)
  session      session/conversation id for grouping
  intent       human-readable description of what the agent is doing
  lines        line numbers being edited (array of ints)
  confidence   0.0-1.0, how confident the agent is about this action
  done         true when agent is finished with this file


action vocabulary
-----------------

core actions (must support):

  edit         modifying file content
  read         reading/analyzing a file
  create       creating a new file
  delete       deleting a file
  rename       renaming (include old path in "old_file" field)

extended actions (optional):

  plan         planning changes to a file
  test         running tests related to a file
  debug        debugging an issue in a file
  review       reviewing/auditing a file
  search       searching within a file
  idle         agent is paused/waiting
  done         agent finished all work (session-level)


pass rate / schema compliance
-----------------------------

not every line needs to match the schema perfectly. the
daemon calculates a pass rate per JSONL source:

  pass_rate = valid_lines / total_lines

thresholds:

  >= 0.8    high compliance
            daemon trusts the data directly
            no LLM calls needed
            active_agent populated from "agent" field
            file attribution from "file" field

  0.4-0.8  medium compliance
            daemon parses what it can
            LLM called on non-conforming lines to extract
            agent/file/action when possible

  < 0.4    low compliance
            not agent activity JSONL, probably something else
            daemon ignores or sends entire chunks to LLM
            for best-effort interpretation

pass rate is calculated on a rolling window (last 100
lines) and cached per source file.


directory registration
----------------------

clients register a directory to watch for agent activity:

  ambient-fsd watch-agents /path/to/agent-output/

or via JSON-RPC:

  {"jsonrpc":"2.0","method":"watch_agents","params":{
    "path": "/path/to/agent-output/",
    "project": "my-project"
  },"id":1}

the daemon then:
  1. watches the directory for new/modified .jsonl files
  2. tails each file (like tail -f, remembers position)
  3. parses new lines against the schema
  4. updates active_agent state in FileAwareness
  5. optionally calls LLM for non-conforming lines

multiple directories can be registered. each maps to a
project (or auto-detected from content).


file layout conventions
-----------------------

recommended directory structure for agent output:

  .agents/
    claude-7.jsonl
    codex-main.jsonl
    opencode-session-abc.jsonl

one file per agent session. filename is the agent id.
daemon creates the .agents/ dir if it doesn't exist when
watch-agents is called.

alternative: single file with agent field distinguishing:

  .agents/activity.jsonl

both work. daemon handles either pattern.


haiku LLM enhancement
---------------------

when pass rate is below threshold, or when "intent" field
is missing and the daemon wants richer context, it calls
a haiku-class model.

input (batch of recent lines from one agent):

  system: you parse AI coding agent activity logs.
          extract: agent_id, file_path, action, intent.
          respond with JSON only.

  user: [raw JSONL lines or freeform agent output]

output:

  {
    "activities": [
      {
        "agent": "claude-7",
        "file": "src/main.rs",
        "action": "edit",
        "intent": "fixing authentication bypass"
      }
    ]
  }

batching: collect non-conforming lines for 5 seconds,
send as one batch. this keeps API calls low.

cost estimate:
  haiku input: ~200 tokens per batch
  haiku output: ~50 tokens per batch
  at $0.25/MTok input, $1.25/MTok output:
  ~$0.00005 per batch + ~$0.00006 per batch
  ~$0.0001 per enhancement call
  at 100 calls/hour = $0.01/hour


daemon integration
------------------

new components needed:

  ambient-fs-server/src/agents.rs
    AgentTracker struct
    - watches registered .agents/ directories
    - tails JSONL files
    - maintains per-agent state:
      HashMap<String, AgentState>
        agent_id -> { files: Vec<String>, last_seen: DateTime,
                      intent: Option<String>, tool: Option<String> }
    - exposes get_active_agent(file_path) -> Option<String>
    - prunes stale agents (no activity for 5 min = inactive)

  ambient-fs-server/src/llm.rs (optional module)
    LlmEnhancer struct
    - batches non-conforming lines
    - calls haiku API
    - parses response back into AgentActivity
    - config: api_key, model, enabled (default false)

  protocol.rs additions:
    Method::WatchAgents
    Method::UnwatchAgents
    Method::QueryAgents (list active agents)

  awareness aggregator changes:
    when building FileAwareness, check AgentTracker for
    active_agent instead of relying on claims.


example flow
------------

  1. user starts ambient-fsd, registers project

  2. user runs: ambient-fsd watch-agents .agents/

  3. claude code starts a session, writes to
     .agents/claude-opus-session-42.jsonl:

     {"ts":1708099200,"agent":"opus-42","action":"read","file":"src/auth.rs","tool":"claude-code"}
     {"ts":1708099205,"agent":"opus-42","action":"edit","file":"src/auth.rs","intent":"fix token validation","tool":"claude-code"}
     {"ts":1708099210,"agent":"opus-42","action":"edit","file":"src/auth.rs","lines":[42,43,44],"tool":"claude-code"}
     {"ts":1708099240,"agent":"opus-42","action":"edit","file":"src/auth.rs","done":true,"tool":"claude-code"}

  4. daemon reads lines, updates AgentTracker:
     active_agent("src/auth.rs") = "opus-42"

  5. kollabor queries awareness for src/auth.rs:
     response includes active_agent: "opus-42",
     intent: "fix token validation"

  6. after done:true or 5 min silence:
     active_agent("src/auth.rs") = None


adapter examples for existing tools
-------------------------------------

these don't exist yet but show how tools could emit
the schema with minimal effort:

claude code (hook in .claude/hooks/):
  post-tool hook that appends to .agents/<session>.jsonl
  when Edit/Write tools are called

codex:
  wrapper script that tees output to .agents/codex.jsonl

opencode:
  plugin that writes activity lines

cursor:
  extension that writes on file save with agent context

the point: any tool that can append a line to a file
can participate. no SDK, no API integration, no protocol
negotiation. just write JSON lines.


what this replaces
------------------

the claim/release protocol in active-agent-protocol.md
is superseded by this approach. advantages:

  claim/release:
    - requires every agent to integrate
    - agents must know about the daemon
    - crash = orphaned claims
    - binary state (claimed / not claimed)

  JSONL watching:
    - zero integration required (just write a file)
    - agents don't need to know about the daemon
    - crash = last line is the last known state, stale
      timeout handles cleanup
    - rich state (action, intent, confidence, lines)
    - works retroactively on existing agent output


open questions
--------------

1. should the daemon also parse non-JSONL formats?
   e.g. plain text agent logs, tmux captures. the LLM
   could handle these but it's more expensive.

2. should the .agents/ directory be inside the project
   or global (~/.local/share/ambient-fs/agents/)?

3. how to handle multiple agents editing the same file?
   show all of them? show the most recent? show the one
   with highest confidence?

4. should the schema be versioned? (e.g. "v": 1)
   probably yes for forward compatibility.

5. rate limiting the LLM: if an agent is very chatty
   and low-compliance, cap at N calls per minute.

6. privacy: agent JSONL might contain code snippets or
   user data. should the daemon sanitize before sending
   to the LLM? or is this local-only by default?
