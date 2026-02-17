kollabor tool_runner attribution hook
======================================

status: design
created: 2026-02-16
affects: kollabor-app-v1 (external, NOT in this repo)


beads: ambient-fs-4wk
----------------------

overview
--------

when Kollabor's tool_runner executes an AI tool that writes
files, or when the voice pipeline creates/modifies files,
those changes need to be attributed to the correct source
(ai_agent or voice) via the ambient-fs daemon.

this is a Kollabor-side integration. ambient-fs provides
the attribution API (9ie, in progress). Kollabor calls it.


note: CLAUDE.md says "Don't touch the Kollabor app"
so this spec documents what Kollabor needs to do, but
implementation happens in the Kollabor repo, not here.


what ambient-fs provides
-------------------------

the attribution API (method: "attribute") accepts:
  {
    "file_path": "src/auth.rs",
    "project_id": "my-project",
    "source": "ai_agent",
    "source_id": "chat_42"
  }

this is exposed via:
  1. Unix socket JSON-RPC (already being implemented)
  2. gRPC (when zwg is done)
  3. tauri-plugin-ambient-fs IPC command (when ch4 is done)
  4. ambient-fs-client Rust API (when 0zp is done)


what Kollabor needs to do
--------------------------

1. in tool_runner.rs, after a tool successfully writes a file:
   call ambient_fs_attribute(project_id, file_path, "ai_agent", chat_id)

2. in the voice pipeline, after transcription writes a file:
   call ambient_fs_attribute(project_id, file_path, "voice", transcription_id)

3. both calls go through tauri-plugin-ambient-fs which
   forwards to the daemon via Unix socket.

see docs/AMBIENT_FS_KOLLABOR_INTEGRATION.md section
"tool_runner Attribution Bridge" for exact code.


what we can do in this repo
-----------------------------

ensure the attribution API works correctly:
  - attribute method in protocol.rs + socket.rs (9ie, in progress)
  - attribute method in client.rs (0zp, in progress)
  - tauri plugin exposes ambient_fs_attribute command (ch4)

test the flow end-to-end:
  - integration test: call attribute via client, verify event
    stored with correct source
  - verify awareness query returns correct modified_by after
    attribution


test strategy
-------------

from ambient-fs side:
  - test attribute API accepts all source types
  - test attributed events appear in query_events
  - test awareness reflects attributed source

from Kollabor side (documented, not implemented here):
  - tool writes file -> daemon receives attribution
  - voice writes file -> daemon receives attribution
  - file tree shows correct source dot color


depends on
----------

  - 9ie attribution API (in progress)
  - ch4 tauri plugin (not started)
  - 0zp client API (in progress)
