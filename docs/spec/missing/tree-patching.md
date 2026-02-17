incremental tree patching
==========================

status: design
created: 2026-02-16
affects: crates/ambient-fs-server/src/tree_state.rs (new),
         crates/ambient-fs-server/src/protocol.rs,
         crates/ambient-fs-server/src/socket.rs


beads: ambient-fs-7t4
----------------------

overview
--------

tree.rs in ambient-fs-core has add_node, remove_node,
rename_node, find_node. these work on a TreeNode struct.
but nothing in the server maintains a live project tree
or exposes it via protocol.

clients (Kollabor) need to query the current tree and
receive incremental patches instead of rescanning the
filesystem on every change.


architecture
------------

the server maintains one TreeNode per watched project.
it's built on startup by scanning the project directory,
then updated incrementally as file events arrive.

  watcher event -> update TreeNode -> broadcast patch

clients can:
  1. query the full tree (initial load)
  2. subscribe to tree patches (incremental updates)

patches are small: { op: "add"|"remove"|"rename", path, is_dir }
instead of sending the whole tree on every change.


implementation
--------------

  crates/ambient-fs-server/src/tree_state.rs (new):

  ProjectTree struct:
    - root: TreeNode
    - project_id: String

  ProjectTree methods:
    - from_directory(path: &Path) -> Result<Self>
      walks directory recursively, builds TreeNode tree
      respects PathFilter (skip .git, node_modules, etc)
    - apply_event(&mut self, event: &FileEvent) -> Option<TreePatch>
      matches on event_type:
        Created -> add_node, returns TreePatch::Add
        Deleted -> remove_node, returns TreePatch::Remove
        Renamed -> rename_node, returns TreePatch::Rename
        Modified -> None (tree structure unchanged)
    - to_tree_node(&self) -> &TreeNode

  TreePatch enum:
    Add { path: String, is_dir: bool }
    Remove { path: String }
    Rename { old_path: String, new_path: String, is_dir: bool }

  both TreePatch and TreeNode are Serialize so they can be
  sent as JSON-RPC responses.


server state integration
--------------------------

  state.rs:
    - add field: trees: Arc<RwLock<HashMap<String, ProjectTree>>>

  when watch_project is called:
    1. scan directory -> build ProjectTree
    2. store in state.trees
    3. subscribe to watcher events
    4. on each event: apply_event, if patch -> broadcast

  when unwatch_project is called:
    - remove from state.trees


protocol additions
-------------------

  protocol.rs:
    - add Method::QueryTree
    - add Method::SubscribeTree (optional, could reuse Subscribe)

  socket.rs handlers:

  QueryTree:
    params: { project_id: "my-project" }
    returns: TreeNode (serialized as JSON)

  the tree is already built in memory, so this is just a
  read from state.trees. no DB query needed.

  tree patches are sent as part of the existing Subscribe
  notification stream. when a tree patch occurs, it's
  broadcast as:
    {"jsonrpc":"2.0","method":"tree_patch","params":{<TreePatch>}}

  this reuses the existing subscription infrastructure.
  clients that don't care about tree patches ignore them.


initial scan
------------

  from_directory needs to walk the filesystem:
    - use walkdir crate or std::fs::read_dir recursive
    - apply PathFilter to skip ignored paths
    - build TreeNode with sorted children (dirs first)
    - cap at max depth (default 20) to avoid infinite recursion
    - cap at max entries (default 10000) to avoid huge trees

  this is a sync operation, so wrap in spawn_blocking when
  called from async context.


test strategy
-------------

unit tests:
  - from_directory builds correct tree from temp dir
  - apply_event Created adds node, returns Add patch
  - apply_event Deleted removes node, returns Remove patch
  - apply_event Renamed moves node, returns Rename patch
  - apply_event Modified returns None
  - PathFilter respected during scan
  - max depth honored
  - QueryTree handler returns serialized tree

integration tests:
  - watch project, create file, verify tree updated
  - watch project, delete file, verify tree updated
  - subscribe, create file, receive tree_patch notification


depends on
----------

  - tree.rs in ambient-fs-core (done, has all low-level ops)
  - ServerState (done)
  - subscribe infrastructure (done)
  - PathFilter (done)
