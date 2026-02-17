use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// A node in the file tree
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    pub fn file(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            is_dir: false,
            children: Vec::new(),
        }
    }

    pub fn dir(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            is_dir: true,
            children: Vec::new(),
        }
    }

    /// Sort order: directories first, then alphabetical (case-insensitive)
    fn sort_key(&self) -> (bool, String) {
        (!self.is_dir, self.name.to_lowercase())
    }

    /// Sort children recursively
    pub fn sort_recursive(&mut self) {
        self.children.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        for child in &mut self.children {
            child.sort_recursive();
        }
    }
}

impl PartialOrd for TreeNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TreeNode {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

/// Add a node to the tree at the correct position.
///
/// Given a file path like "src/components/App.vue", this creates
/// intermediate directories as needed and inserts the file node
/// in sorted order.
///
/// Returns true if the node was added, false if it already existed.
pub fn add_node(root: &mut TreeNode, file_path: &str, is_dir: bool) -> bool {
    let parts: Vec<&str> = file_path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return false;
    }
    add_node_recursive(root, &parts, file_path, is_dir)
}

fn add_node_recursive(
    parent: &mut TreeNode,
    parts: &[&str],
    full_path: &str,
    is_dir: bool,
) -> bool {
    if parts.is_empty() {
        return false;
    }

    let name = parts[0];
    let is_leaf = parts.len() == 1;

    // Check if this component already exists
    if let Some(existing) = parent.children.iter_mut().find(|c| c.name == name) {
        if is_leaf {
            // Already exists
            return false;
        }
        // Recurse into existing directory
        return add_node_recursive(existing, &parts[1..], full_path, is_dir);
    }

    if is_leaf {
        // Create the leaf node
        let node = if is_dir {
            TreeNode::dir(name, full_path)
        } else {
            TreeNode::file(name, full_path)
        };
        insert_sorted(&mut parent.children, node);
        true
    } else {
        // Create intermediate directory
        let path_so_far = build_path(full_path, parts.len());
        let mut dir_node = TreeNode::dir(name, path_so_far);
        let result = add_node_recursive(&mut dir_node, &parts[1..], full_path, is_dir);
        insert_sorted(&mut parent.children, dir_node);
        result
    }
}

/// Remove a node from the tree by path.
///
/// Returns true if the node was found and removed.
pub fn remove_node(root: &mut TreeNode, file_path: &str) -> bool {
    let parts: Vec<&str> = file_path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return false;
    }
    remove_node_recursive(root, &parts)
}

fn remove_node_recursive(parent: &mut TreeNode, parts: &[&str]) -> bool {
    if parts.is_empty() {
        return false;
    }

    let name = parts[0];
    let is_leaf = parts.len() == 1;

    if is_leaf {
        let before = parent.children.len();
        parent.children.retain(|c| c.name != name);
        parent.children.len() < before
    } else {
        if let Some(child) = parent.children.iter_mut().find(|c| c.name == name) {
            remove_node_recursive(child, &parts[1..])
        } else {
            false
        }
    }
}

/// Rename a node in the tree.
///
/// Removes the node at old_path and adds it at new_path.
/// Returns true if the rename was successful.
pub fn rename_node(root: &mut TreeNode, old_path: &str, new_path: &str, is_dir: bool) -> bool {
    let removed = remove_node(root, old_path);
    if !removed {
        return false;
    }
    add_node(root, new_path, is_dir)
}

/// Find a node by path
pub fn find_node<'a>(root: &'a TreeNode, file_path: &str) -> Option<&'a TreeNode> {
    let parts: Vec<&str> = file_path.split('/').filter(|s| !s.is_empty()).collect();
    find_node_recursive(root, &parts)
}

fn find_node_recursive<'a>(parent: &'a TreeNode, parts: &[&str]) -> Option<&'a TreeNode> {
    if parts.is_empty() {
        return Some(parent);
    }

    let name = parts[0];
    parent
        .children
        .iter()
        .find(|c| c.name == name)
        .and_then(|child| {
            if parts.len() == 1 {
                Some(child)
            } else {
                find_node_recursive(child, &parts[1..])
            }
        })
}

/// Insert a node in sorted position (dirs first, then alphabetical)
fn insert_sorted(children: &mut Vec<TreeNode>, node: TreeNode) {
    let pos = children
        .binary_search_by(|existing| existing.sort_key().cmp(&node.sort_key()))
        .unwrap_or_else(|pos| pos);
    children.insert(pos, node);
}

/// Build a partial path from full_path, using the first N components
/// from the end (remaining_parts tells us how many are left)
fn build_path(full_path: &str, remaining_parts: usize) -> String {
    let all_parts: Vec<&str> = full_path.split('/').filter(|s| !s.is_empty()).collect();
    let take = all_parts.len() - remaining_parts + 1;
    all_parts[..take].join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn make_root() -> TreeNode {
        TreeNode::dir("root", "")
    }

    #[test]
    fn add_single_file() {
        let mut root = make_root();
        assert!(add_node(&mut root, "README.md", false));
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].name, "README.md");
        assert!(!root.children[0].is_dir);
    }

    #[test]
    fn add_nested_file_creates_intermediate_dirs() {
        let mut root = make_root();
        assert!(add_node(&mut root, "src/components/App.vue", false));

        assert_eq!(root.children.len(), 1);
        let src = &root.children[0];
        assert_eq!(src.name, "src");
        assert!(src.is_dir);

        let components = &src.children[0];
        assert_eq!(components.name, "components");
        assert!(components.is_dir);

        let app = &components.children[0];
        assert_eq!(app.name, "App.vue");
        assert!(!app.is_dir);
    }

    #[test]
    fn add_duplicate_returns_false() {
        let mut root = make_root();
        assert!(add_node(&mut root, "README.md", false));
        assert!(!add_node(&mut root, "README.md", false));
        assert_eq!(root.children.len(), 1);
    }

    #[test]
    fn add_preserves_sorted_order() {
        let mut root = make_root();
        add_node(&mut root, "zebra.txt", false);
        add_node(&mut root, "alpha.txt", false);
        add_node(&mut root, "middle.txt", false);

        let names: Vec<&str> = root.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["alpha.txt", "middle.txt", "zebra.txt"]);
    }

    #[test]
    fn dirs_sort_before_files() {
        let mut root = make_root();
        add_node(&mut root, "file.txt", false);
        add_node(&mut root, "src/main.rs", false);
        add_node(&mut root, "aaa.txt", false);

        // src/ dir should come before files
        assert!(root.children[0].is_dir);
        assert_eq!(root.children[0].name, "src");
        assert_eq!(root.children[1].name, "aaa.txt");
        assert_eq!(root.children[2].name, "file.txt");
    }

    #[test]
    fn remove_file() {
        let mut root = make_root();
        add_node(&mut root, "src/main.rs", false);
        add_node(&mut root, "src/lib.rs", false);

        assert!(remove_node(&mut root, "src/main.rs"));

        let src = &root.children[0];
        assert_eq!(src.children.len(), 1);
        assert_eq!(src.children[0].name, "lib.rs");
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let mut root = make_root();
        assert!(!remove_node(&mut root, "nope.txt"));
    }

    #[test]
    fn remove_deeply_nested() {
        let mut root = make_root();
        add_node(&mut root, "a/b/c/d.txt", false);
        assert!(remove_node(&mut root, "a/b/c/d.txt"));

        // parent dirs remain (no auto-prune)
        let a = &root.children[0];
        assert_eq!(a.name, "a");
    }

    #[test]
    fn rename_file() {
        let mut root = make_root();
        add_node(&mut root, "src/old.rs", false);

        assert!(rename_node(&mut root, "src/old.rs", "src/new.rs", false));

        let found = find_node(&root, "src/new.rs");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "new.rs");

        let old = find_node(&root, "src/old.rs");
        assert!(old.is_none());
    }

    #[test]
    fn rename_to_different_directory() {
        let mut root = make_root();
        add_node(&mut root, "src/old.rs", false);

        assert!(rename_node(&mut root, "src/old.rs", "lib/new.rs", false));

        assert!(find_node(&root, "lib/new.rs").is_some());
        assert!(find_node(&root, "src/old.rs").is_none());
    }

    #[test]
    fn rename_nonexistent_returns_false() {
        let mut root = make_root();
        assert!(!rename_node(&mut root, "nope.rs", "other.rs", false));
    }

    #[test]
    fn find_node_existing() {
        let mut root = make_root();
        add_node(&mut root, "src/components/Button.vue", false);

        let found = find_node(&root, "src/components/Button.vue");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "Button.vue");
    }

    #[test]
    fn find_node_directory() {
        let mut root = make_root();
        add_node(&mut root, "src/components/Button.vue", false);

        let found = find_node(&root, "src/components");
        assert!(found.is_some());
        assert!(found.unwrap().is_dir);
    }

    #[test]
    fn find_node_nonexistent() {
        let root = make_root();
        assert!(find_node(&root, "nope.txt").is_none());
    }

    #[test]
    fn add_empty_path_returns_false() {
        let mut root = make_root();
        assert!(!add_node(&mut root, "", false));
    }

    #[test]
    fn add_directory_node() {
        let mut root = make_root();
        assert!(add_node(&mut root, "src/components", true));

        let found = find_node(&root, "src/components");
        assert!(found.is_some());
        assert!(found.unwrap().is_dir);
    }

    #[test]
    fn sort_key_case_insensitive() {
        let mut root = make_root();
        add_node(&mut root, "Zebra.md", false);
        add_node(&mut root, "alpha.md", false);

        let names: Vec<&str> = root.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["alpha.md", "Zebra.md"]);
    }

    #[test]
    fn tree_node_serde_roundtrip() {
        let mut root = make_root();
        add_node(&mut root, "src/main.rs", false);
        add_node(&mut root, "src/lib.rs", false);
        add_node(&mut root, "README.md", false);

        let json = serde_json::to_string(&root).unwrap();
        let deserialized: TreeNode = serde_json::from_str(&json).unwrap();
        assert_eq!(root, deserialized);
    }

    #[test]
    fn multiple_files_in_same_directory() {
        let mut root = make_root();
        add_node(&mut root, "src/a.rs", false);
        add_node(&mut root, "src/b.rs", false);
        add_node(&mut root, "src/c.rs", false);

        let src = find_node(&root, "src").unwrap();
        assert_eq!(src.children.len(), 3);
        let names: Vec<&str> = src.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a.rs", "b.rs", "c.rs"]);
    }
}
