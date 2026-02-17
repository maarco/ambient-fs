chat references counting
=========================

status: design
created: 2026-02-16
affects: crates/ambient-fs-analyzer/src/analyzer.rs,
         crates/ambient-fs-server/src/awareness.rs


beads: ambient-fs-ynk
----------------------

overview
--------

FileAwareness has a chat_references field (u32) that's
always 0. it should count how many times a file is
referenced in AI chat sessions. this tells users which
files are "hot" in conversations.


design: LLM-enhanced approach
-------------------------------

following the same innovative pattern as the analyzer
(single LLM call for multiple signals), chat reference
counting can be done as part of the agent activity
protocol rather than the file analyzer.

when an agent's JSONL activity mentions a file path,
that's a chat reference. the agent tracker already
parses these lines.


two sources of chat references
--------------------------------

1. agent activity JSONL (passive, already parsed)
   when an agent reads/edits/plans a file, that's a
   reference. the AgentTracker already tracks this.

   count = number of unique JSONL lines mentioning
   this file in the last N hours (configurable).

2. explicit chat log scanning (active, optional)
   scan a directory of chat logs for file path mentions.
   this would use the same LLM approach: send recent
   chat lines to haiku, ask "which file paths are
   mentioned?", count occurrences.

   this is more expensive and complex. defer to v2.


implementation (v1: agent activity based)
-------------------------------------------

  crates/ambient-fs-server/src/agents.rs:

  AgentTracker additions:
    - track file_references: HashMap<String, u32>
      (file_path -> reference count)
    - in update_from_activity():
      increment file_references[activity.file] += 1
    - add get_reference_count(file_path) -> u32
    - prune_stale() also resets counts for stale agents

  crates/ambient-fs-server/src/awareness.rs:

  build_awareness() additions:
    - after getting active_agent, also get reference count:
      let refs = state.agent_tracker.get_reference_count(file_path).await;
    - set awareness.chat_references = refs as u32

  this is simple because the data already flows through
  the agent tracker. we just need to count it.


implementation (v2: chat log scanning, future)
-------------------------------------------------

  a separate ChatLogScanner that:
  1. watches a directory of chat log files
  2. tails new content
  3. sends batches to haiku LLM:
     "extract all file paths mentioned in these chat lines"
  4. updates a reference count map
  5. shared with awareness aggregator

  uses the same LLM client infrastructure as LlmAnalyzer.
  cost: same order of magnitude (~$0.01/hour).

  this is deferred. v1 with agent activity counts is
  sufficient to make the field non-zero and useful.


test strategy
-------------

v1 tests:
  - agent activity with file -> reference count incremented
  - multiple activities for same file -> count accumulates
  - prune stale -> counts reset
  - build_awareness includes chat_references from tracker
  - no agent activity -> chat_references = 0

v2 tests (future):
  - chat log with file path -> counted
  - multiple mentions in one chat -> counted once per line
  - LLM extraction matches expected paths


depends on
----------

  - AgentTracker (done, Phase2-AgentTracker)
  - awareness aggregator (hra, in progress)
  - LLM client (done, Phase1-LlmClient) -- for v2 only
