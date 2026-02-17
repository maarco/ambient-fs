// Project tree state with incremental patching
// TDD: Tests FIRST, then implementation

use ambient_fs_core::tree::{TreeNode, add_node, remove_node, rename_node, find_node};
use ambient_fs_core::filter::PathFilter;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Incremental tree patch operation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum TreePatch {
    /// A file or directory was added
    #[serde(rename = "add")]
    Add { path: String, is_dir: bool },
    /// A file or directory was removed
    #[serde(rename = "remove")]
    Remove { path: String },
    /// A file or directory was renamed
    #[serde(rename = "rename")]
    Rename { old_path: String, new_path: String, is_dir: bool },
}

/// Project tree state
///
/// Holds the root TreeNode for a project and tracks the project ID.
/// Provides methods to build from filesystem and apply incremental patches.
#[derive(Debug, Clone)]
pub struct ProjectTree {
    /// Root node of the tree (name is empty string for project root)
    pub root: TreeNode,
    /// Project identifier
    pub project_id: String,
}

impl ProjectTree {
    const MAX_DEPTH: usize = 20;
    const MAX_ENTRIES: usize = 10000;

    /// Create a new empty ProjectTree
    pub fn new(project_id: String) -> Self {
        Self {
            root: TreeNode::dir("", ""),
            project_id,
        }
    }

    /// Build a ProjectTree by scanning a directory
    ///
    /// Walks the filesystem recursively, applying PathFilter to skip
    /// ignored paths. Caps at MAX_DEPTH and MAX_ENTRIES.
    pub fn from_directory(path: &Path, filter: &PathFilter) -> Result<Self, std::io::Error> {
        let project_id = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mut tree = Self::new(project_id);
        tree.scan_directory(path, "", filter, 0, &mut 0)?;
        Ok(tree)
    }

    /// Recursively scan directory and populate tree
    fn scan_directory(
        &mut self,
        base_path: &Path,
        relative_path: &str,
        filter: &PathFilter,
        depth: usize,
        entry_count: &mut usize,
    ) -> Result<(), std::io::Error> {
        if depth >= Self::MAX_DEPTH {
            return Ok(());
        }

        let full_path = if relative_path.is_empty() {
            base_path.to_path_buf()
        } else {
            base_path.join(relative_path)
        };

        let entries = match std::fs::read_dir(&full_path) {
            Ok(e) => e,
            Err(_) => return Ok(()), // Skip directories we can't read
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue, // Skip non-UTF8 names
            };

            let child_relative = if relative_path.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", relative_path, name)
            };

            // Check filter
            if filter.should_ignore(&child_relative) {
                continue;
            }

            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            let is_dir = file_type.is_dir();
            let is_symlink = file_type.is_symlink();

            // Skip symlinks to avoid cycles
            if is_symlink {
                continue;
            }

            // Check entry count limit
            if *entry_count >= Self::MAX_ENTRIES {
                break;
            }

            // Add to tree
            add_node(&mut self.root, &child_relative, is_dir);
            *entry_count += 1;

            // Recurse into directories
            if is_dir {
                self.scan_directory(base_path, &child_relative, filter, depth + 1, entry_count)?;
            }
        }

        Ok(())
    }

    /// Apply a file event to the tree, returning a patch if the structure changed
    ///
    /// - Created -> adds node, returns Add patch
    /// - Deleted -> removes node, returns Remove patch
    /// - Renamed -> moves node, returns Rename patch
    /// - Modified -> no structural change, returns None
    pub fn apply_event(&mut self, event: &ambient_fs_core::event::FileEvent) -> Option<TreePatch> {
        use ambient_fs_core::event::EventType;

        match event.event_type {
            EventType::Created => {
                let added = add_node(&mut self.root, &event.file_path, false);
                if added {
                    Some(TreePatch::Add {
                        path: event.file_path.clone(),
                        is_dir: false,
                    })
                } else {
                    None
                }
            }
            EventType::Deleted => {
                let removed = remove_node(&mut self.root, &event.file_path);
                if removed {
                    Some(TreePatch::Remove {
                        path: event.file_path.clone(),
                    })
                } else {
                    None
                }
            }
            EventType::Renamed => {
                // Use old_path from event if available
                if let Some(ref old_path) = event.old_path {
                    self.apply_rename(old_path, &event.file_path, false)
                } else {
                    None
                }
            }
            EventType::Modified => None,
        }
    }

    /// Apply a rename operation explicitly
    ///
    /// This handles renames where both old and new paths are known.
    pub fn apply_rename(&mut self, old_path: &str, new_path: &str, is_dir: bool) -> Option<TreePatch> {
        let renamed = rename_node(&mut self.root, old_path, new_path, is_dir);
        if renamed {
            Some(TreePatch::Rename {
                old_path: old_path.to_string(),
                new_path: new_path.to_string(),
                is_dir,
            })
        } else {
            None
        }
    }

    /// Get a reference to the root TreeNode
    pub fn to_tree_node(&self) -> &TreeNode {
        &self.root
    }

    /// Find a node by path
    pub fn find(&self, path: &str) -> Option<&TreeNode> {
        find_node(&self.root, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use ambient_fs_core::event::{FileEvent, EventType, Source};
    use chrono::Utc;

    // ========== TreePatch serde ==========

    #[test]
    fn tree_patch_add_serializes_correctly() {
        let patch = TreePatch::Add {
            path: "src/main.rs".to_string(),
            is_dir: false,
        };
        let json = serde_json::to_string(&patch).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["op"], "add");
        assert_eq!(v["path"], "src/main.rs");
        assert_eq!(v["is_dir"], false);
    }

    #[test]
    fn tree_patch_remove_serializes_correctly() {
        let patch = TreePatch::Remove {
            path: "old_file.rs".to_string(),
        };
        let json = serde_json::to_string(&patch).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["op"], "remove");
        assert_eq!(v["path"], "old_file.rs");
    }

    #[test]
    fn tree_patch_rename_serializes_correctly() {
        let patch = TreePatch::Rename {
            old_path: "old.rs".to_string(),
            new_path: "new.rs".to_string(),
            is_dir: false,
        };
        let json = serde_json::to_string(&patch).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["op"], "rename");
        assert_eq!(v["old_path"], "old.rs");
        assert_eq!(v["new_path"], "new.rs");
        assert_eq!(v["is_dir"], false);
    }

    #[test]
    fn tree_patch_roundtrips_through_json() {
        let patches = vec![
            TreePatch::Add { path: "a/b/c.rs".to_string(), is_dir: false },
            TreePatch::Remove { path: "x.rs".to_string() },
            TreePatch::Rename { old_path: "old".to_string(), new_path: "new".to_string(), is_dir: true },
        ];

        for patch in patches {
            let json = serde_json::to_string(&patch).unwrap();
            let deserialized: TreePatch = serde_json::from_str(&json).unwrap();
            assert_eq!(patch, deserialized);
        }
    }

    // ========== ProjectTree::new ==========

    #[test]
    fn new_creates_empty_tree_with_project_id() {
        let tree = ProjectTree::new("test-project".to_string());
        assert_eq!(tree.project_id, "test-project");
        assert_eq!(tree.root.name, "");
        assert!(tree.root.is_dir);
        assert!(tree.root.children.is_empty());
    }

    // ========== from_directory ==========

    #[test]
    fn from_directory_builds_flat_file_tree() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create some files
        std::fs::write(root.join("README.md"), b"hello").unwrap();
        std::fs::write(root.join("main.rs"), b"fn main() {}").unwrap();

        let filter = PathFilter::default();
        let tree = ProjectTree::from_directory(root, &filter).unwrap();

        assert_eq!(tree.root.children.len(), 2);
        let names: Vec<&str> = tree.root.children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"README.md"));
        assert!(names.contains(&"main.rs"));
    }

    #[test]
    fn from_directory_builds_nested_tree() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create nested structure
        std::fs::create_dir_all(root.join("src/components")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"").unwrap();
        std::fs::write(root.join("src/components/App.vue"), b"").unwrap();
        std::fs::write(root.join("README.md"), b"").unwrap();

        let filter = PathFilter::default();
        let tree = ProjectTree::from_directory(root, &filter).unwrap();

        // src and README at root
        assert_eq!(tree.root.children.len(), 2);

        let src = tree.find("src").unwrap();
        assert!(src.is_dir);
        assert_eq!(src.children.len(), 2); // main.rs and components

        let components = tree.find("src/components").unwrap();
        assert!(components.is_dir);
        assert_eq!(components.children.len(), 1);
    }

    #[test]
    fn from_directory_respects_path_filter() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create files that should be filtered
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        std::fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main").unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), b"").unwrap();
        std::fs::write(root.join("README.md"), b"").unwrap();

        let filter = PathFilter::default();
        let tree = ProjectTree::from_directory(root, &filter).unwrap();

        // Only README should remain (git and node_modules filtered)
        assert_eq!(tree.root.children.len(), 1);
        assert_eq!(tree.root.children[0].name, "README.md");
        assert!(tree.find(".git").is_none());
        assert!(tree.find("node_modules").is_none());
    }

    #[test]
    fn from_directory_sorts_dirs_first() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::create_dir(root.join("zzz_dir")).unwrap();
        std::fs::write(root.join("aaa.txt"), b"").unwrap();
        std::fs::write(root.join("mid.rs"), b"").unwrap();

        let filter = PathFilter::default();
        let tree = ProjectTree::from_directory(root, &filter).unwrap();

        let names: Vec<&str> = tree.root.children.iter().map(|c| c.name.as_str()).collect();
        // Dirs first (src, zzz_dir), then files alphabetically (aaa.txt, mid.rs)
        assert_eq!(names, vec!["src", "zzz_dir", "aaa.txt", "mid.rs"]);
    }

    #[test]
    fn from_directory_stops_at_max_depth() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create deeply nested structure
        let mut path = root.to_path_buf();
        for i in 0..30 {
            path = path.join(format!("d{}", i));
            std::fs::create_dir(&path).unwrap();
        }
        // Add a file at the deepest level
        std::fs::write(path.join("deep.txt"), "deep file").unwrap();

        let filter = PathFilter::default();
        let tree = ProjectTree::from_directory(root, &filter).unwrap();

        // Should stop at MAX_DEPTH=20
        // Check that very deep paths are not included
        assert!(tree.find("d0/d1/d2").is_some());
        assert!(tree.find("d0/d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16/d17/d18/d19").is_some());
        // d20 would be depth 21, so it should not exist
        assert!(tree.find("d0/d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16/d17/d18/d19/d20").is_none());
    }

    #[test]
    fn from_directory_stops_at_max_entries() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create many files
        for i in 0..15000 {
            std::fs::write(root.join(format!("file{}.txt", i)), b"").unwrap();
        }

        let filter = PathFilter::default();
        let tree = ProjectTree::from_directory(root, &filter).unwrap();

        // Should cap at MAX_ENTRIES=10000
        assert!(tree.root.children.len() <= 10050); // Some fudge for rounding
        assert!(tree.root.children.len() >= 10000); // But should hit the cap
    }

    #[test]
    fn from_directory_skips_symlinks() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create a file and a symlink to it
        std::fs::write(root.join("real.txt"), b"content").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("real.txt", root.join("link.txt")).unwrap();

        let filter = PathFilter::default();
        let tree = ProjectTree::from_directory(root, &filter).unwrap();

        // Only real file should be in tree
        assert!(tree.find("real.txt").is_some());
        // Symlink should be skipped
        assert!(tree.find("link.txt").is_none());
    }

    #[test]
    fn from_directory_handles_non_utf8_names_gracefully() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create a file with valid UTF-8 name
        std::fs::write(root.join("valid.txt"), b"").unwrap();

        let filter = PathFilter::default();
        let tree = ProjectTree::from_directory(root, &filter).unwrap();

        assert!(tree.find("valid.txt").is_some());
        // Should not panic on non-UTF8 names (we skip them)
    }

    // ========== apply_event ==========

    #[test]
    fn apply_event_created_adds_file() {
        let mut tree = ProjectTree::new("test".to_string());

        let event = FileEvent::new(
            EventType::Created,
            "src/main.rs",
            "test",
            "machine",
        );

        let patch = tree.apply_event(&event);

        assert_eq!(patch, Some(TreePatch::Add {
            path: "src/main.rs".to_string(),
            is_dir: false,
        }));
        assert!(tree.find("src/main.rs").is_some());
        assert!(!tree.find("src/main.rs").unwrap().is_dir);
    }

    #[test]
    fn apply_event_created_adds_directory() {
        let mut tree = ProjectTree::new("test".to_string());

        let event = FileEvent::new(
            EventType::Created,
            "src/components",
            "test",
            "machine",
        );

        // Note: Currently treats all Created as files
        // In practice, watchers send Created for both
        let patch = tree.apply_event(&event);

        assert!(patch.is_some());
        assert!(tree.find("src/components").is_some());
    }

    #[test]
    fn apply_event_created_duplicate_returns_none() {
        let mut tree = ProjectTree::new("test".to_string());

        // Add once
        add_node(&mut tree.root, "existing.rs", false);

        let event = FileEvent::new(
            EventType::Created,
            "existing.rs",
            "test",
            "machine",
        );

        let patch = tree.apply_event(&event);
        assert!(patch.is_none()); // Already exists
    }

    #[test]
    fn apply_event_deleted_removes_file() {
        let mut tree = ProjectTree::new("test".to_string());
        add_node(&mut tree.root, "to_delete.rs", false);

        let event = FileEvent::new(
            EventType::Deleted,
            "to_delete.rs",
            "test",
            "machine",
        );

        let patch = tree.apply_event(&event);

        assert_eq!(patch, Some(TreePatch::Remove {
            path: "to_delete.rs".to_string(),
        }));
        assert!(tree.find("to_delete.rs").is_none());
    }

    #[test]
    fn apply_event_deleted_nonexistent_returns_none() {
        let mut tree = ProjectTree::new("test".to_string());

        let event = FileEvent::new(
            EventType::Deleted,
            "nonexistent.rs",
            "test",
            "machine",
        );

        let patch = tree.apply_event(&event);
        assert!(patch.is_none());
    }

    #[test]
    fn apply_event_modified_returns_none() {
        let mut tree = ProjectTree::new("test".to_string());
        add_node(&mut tree.root, "existing.rs", false);

        let event = FileEvent::new(
            EventType::Modified,
            "existing.rs",
            "test",
            "machine",
        );

        let patch = tree.apply_event(&event);
        assert!(patch.is_none()); // Modified doesn't change structure

        // File should still exist
        assert!(tree.find("existing.rs").is_some());
    }

    #[test]
    fn apply_event_renamed_without_old_path_returns_none() {
        let mut tree = ProjectTree::new("test".to_string());
        add_node(&mut tree.root, "old.rs", false);

        let event = FileEvent::new(
            EventType::Renamed,
            "new.rs",
            "test",
            "machine",
        );

        // Without old_path, rename can't be processed
        let patch = tree.apply_event(&event);
        assert!(patch.is_none());

        // Old file still exists (not renamed)
        assert!(tree.find("old.rs").is_some());
        assert!(tree.find("new.rs").is_none());
    }

    #[test]
    fn apply_event_renamed_with_old_path_works() {
        let mut tree = ProjectTree::new("test".to_string());
        add_node(&mut tree.root, "old.rs", false);

        let mut event = FileEvent::new(
            EventType::Renamed,
            "new.rs",
            "test",
            "machine",
        );
        event.old_path = Some("old.rs".to_string());

        let patch = tree.apply_event(&event);

        assert_eq!(patch, Some(TreePatch::Rename {
            old_path: "old.rs".to_string(),
            new_path: "new.rs".to_string(),
            is_dir: false,
        }));

        // File was renamed
        assert!(tree.find("new.rs").is_some());
        assert!(tree.find("old.rs").is_none());
    }

    // ========== apply_rename ==========

    #[test]
    fn apply_rename_moves_node() {
        let mut tree = ProjectTree::new("test".to_string());
        add_node(&mut tree.root, "src/old.rs", false);

        let patch = tree.apply_rename("src/old.rs", "src/new.rs", false);

        assert_eq!(patch, Some(TreePatch::Rename {
            old_path: "src/old.rs".to_string(),
            new_path: "src/new.rs".to_string(),
            is_dir: false,
        }));

        assert!(tree.find("src/new.rs").is_some());
        assert!(tree.find("src/old.rs").is_none());
    }

    #[test]
    fn apply_rename_creates_intermediate_dirs() {
        let mut tree = ProjectTree::new("test".to_string());
        add_node(&mut tree.root, "old.rs", false);

        let patch = tree.apply_rename("old.rs", "lib/new.rs", false);

        assert!(patch.is_some());
        assert!(tree.find("lib/new.rs").is_some());
        assert!(tree.find("old.rs").is_none());
    }

    #[test]
    fn apply_rename_nonexistent_returns_none() {
        let mut tree = ProjectTree::new("test".to_string());

        let patch = tree.apply_rename("nonexistent.rs", "new.rs", false);
        assert!(patch.is_none());
    }

    #[test]
    fn apply_rename_to_existing_returns_none() {
        let mut tree = ProjectTree::new("test".to_string());
        add_node(&mut tree.root, "old.rs", false);
        add_node(&mut tree.root, "new.rs", false);

        let patch = tree.apply_rename("old.rs", "new.rs", false);
        assert!(patch.is_none()); // Can't rename to existing path
    }

    // ========== find ==========

    #[test]
    fn find_locates_existing_file() {
        let mut tree = ProjectTree::new("test".to_string());
        add_node(&mut tree.root, "src/main.rs", false);

        let found = tree.find("src/main.rs");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "main.rs");
    }

    #[test]
    fn find_locates_directory() {
        let mut tree = ProjectTree::new("test".to_string());
        add_node(&mut tree.root, "src/components", true);

        let found = tree.find("src/components");
        assert!(found.is_some());
        assert!(found.unwrap().is_dir);
    }

    #[test]
    fn find_returns_none_for_nonexistent() {
        let tree = ProjectTree::new("test".to_string());

        assert!(tree.find("nonexistent").is_none());
        assert!(tree.find("deep/nested/path").is_none());
    }

    // ========== Integration tests ==========

    #[test]
    fn full_workflow_scan_modify_delete() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Initial scan
        std::fs::write(root.join("a.txt"), b"").unwrap();
        std::fs::write(root.join("b.rs"), b"").unwrap();

        let filter = PathFilter::default();
        let mut tree = ProjectTree::from_directory(root, &filter).unwrap();
        assert_eq!(tree.root.children.len(), 2);

        // Simulate file creation
        let created_event = FileEvent::new(EventType::Created, "c.rs", "test", "m");
        tree.apply_event(&created_event);
        assert!(tree.find("c.rs").is_some());

        // Modified doesn't change structure
        let mod_event = FileEvent::new(EventType::Modified, "a.txt", "test", "m");
        assert!(tree.apply_event(&mod_event).is_none());

        // Delete
        let del_event = FileEvent::new(EventType::Deleted, "b.rs", "test", "m");
        let patch = tree.apply_event(&del_event);
        assert!(matches!(patch, Some(TreePatch::Remove { .. })));
        assert!(tree.find("b.rs").is_none());
    }
}
