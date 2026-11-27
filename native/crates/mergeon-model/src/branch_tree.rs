//! Slash-separated branch names organized into collapsible folders.
//!
//! Pure data transformation with no UI dependencies: takes any list of branch
//! items, groups multi-segment names (`feature/auth/oauth`) into nested
//! [`BranchTreeNode::Folder`]s with leaf [`BranchTreeNode::Branch`]es, and can
//! flatten the tree into visible rows for virtualized rendering.

use std::collections::HashMap;

use crate::collate::locale_compare;

/// A node in a hierarchical branch tree built from slash-separated branch names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchTreeNode<T> {
    Folder {
        /// Full slash-joined folder path, e.g. `"feature"` or `"feature/auth"`.
        path: String,
        /// Segment label for this folder, e.g. `"feature"` or `"auth"`.
        label: String,
        /// Total leaf branches inside this folder subtree.
        branch_count: usize,
        /// True if any branch inside this subtree is active (HEAD).
        contains_active: bool,
        children: Vec<BranchTreeNode<T>>,
    },
    Branch {
        /// Leaf display name after the last `/`, e.g. `"login"`.
        label: String,
        /// Original item carrying the full branch name (`"feature/login"`).
        item: T,
    },
}

/// A flattened visible row produced from a [`BranchTreeNode`] forest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchTreeRow<T> {
    Folder {
        /// Full slash-joined folder path, e.g. `"feature"` or `"feature/auth"`.
        path: String,
        /// Segment label for this folder, e.g. `"feature"` or `"auth"`.
        label: String,
        /// Total leaf branches inside this folder subtree.
        branch_count: usize,
        /// True if any branch inside this subtree is active (HEAD).
        contains_active: bool,
        /// Nesting depth (`0` for top-level folders).
        depth: usize,
        /// Whether this folder's children are currently visible.
        expanded: bool,
    },
    Branch {
        /// Leaf display name after the last `/`, e.g. `"login"`.
        label: String,
        /// Nesting depth (`0` for root branches, `>= 1` inside folders).
        depth: usize,
        /// Original item carrying the full branch name.
        item: T,
    },
}

struct FolderBuilder<T> {
    path: String,
    label: String,
    branches: Vec<(String, T, bool)>,
    folders: Vec<FolderBuilder<T>>,
    folder_index: HashMap<String, usize>,
}

impl<T> FolderBuilder<T> {
    fn new(path: String, label: String) -> Self {
        Self {
            path,
            label,
            branches: Vec::new(),
            folders: Vec::new(),
            folder_index: HashMap::new(),
        }
    }

    fn get_or_insert_folder(&mut self, segment: &str) -> &mut FolderBuilder<T> {
        let index = if let Some(&existing) = self.folder_index.get(segment) {
            existing
        } else {
            let path = if self.path.is_empty() {
                segment.to_string()
            } else {
                format!("{}/{segment}", self.path)
            };
            let index = self.folders.len();
            self.folders
                .push(FolderBuilder::new(path, segment.to_string()));
            self.folder_index.insert(segment.to_string(), index);
            index
        };
        &mut self.folders[index]
    }

    fn finish_children(mut self) -> (Vec<BranchTreeNode<T>>, usize, bool) {
        let mut children = Vec::with_capacity(self.branches.len() + self.folders.len());
        let mut branch_count = 0usize;
        let mut contains_active = false;

        // Leaf branches first in their existing order (keeps `main` at the top).
        for (label, item, active) in self.branches {
            branch_count += 1;
            contains_active |= active;
            children.push(BranchTreeNode::Branch { label, item });
        }

        // Subfolders after leaf branches, sorted alphabetically case-insensitively.
        self.folders
            .sort_by(|left, right| locale_compare(&left.label, &right.label));
        for folder in self.folders {
            let path = folder.path.clone();
            let label = folder.label.clone();
            let (nested_children, nested_count, nested_active) = folder.finish_children();
            branch_count += nested_count;
            contains_active |= nested_active;
            children.push(BranchTreeNode::Folder {
                path,
                label,
                branch_count: nested_count,
                contains_active: nested_active,
                children: nested_children,
            });
        }

        (children, branch_count, contains_active)
    }
}

/// Groups a list of branches into a slash-separated folder tree.
///
/// Branches with fewer than two non-empty `/`-separated segments stay at the
/// root with `label = name_of(&item)`. At every level, leaf branches appear
/// first (in their original input order) followed by folders sorted
/// alphabetically case-insensitively.
pub fn build_branch_tree<T, S, FName, FActive>(
    items: Vec<T>,
    name_of: FName,
    is_active: FActive,
) -> Vec<BranchTreeNode<T>>
where
    S: AsRef<str>,
    FName: Fn(&T) -> S,
    FActive: Fn(&T) -> bool,
{
    let mut root = FolderBuilder::new(String::new(), String::new());

    for item in items {
        let active = is_active(&item);
        let name = name_of(&item).as_ref().to_string();
        let segments: Vec<&str> = name
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect();

        if segments.len() <= 1 {
            root.branches.push((name, item, active));
            continue;
        }

        let (leaf, folder_segments) = segments.split_last().expect("at least 2 segments");
        let leaf_label = (*leaf).to_string();
        let mut current = &mut root;
        for segment in folder_segments {
            current = current.get_or_insert_folder(segment);
        }
        current.branches.push((leaf_label, item, active));
    }

    root.finish_children().0
}

/// Flattens a branch tree into visible rows in display order.
///
/// `is_expanded` receives each folder's full `path` (e.g. `"feature"` or
/// `"feature/auth"`). Collapsed folders emit their own row and omit their
/// descendants.
pub fn flatten_branch_tree<T, FExpanded>(
    nodes: &[BranchTreeNode<T>],
    is_expanded: FExpanded,
) -> Vec<BranchTreeRow<T>>
where
    T: Clone,
    FExpanded: Fn(&str) -> bool,
{
    let mut rows = Vec::new();
    let mut stack: Vec<(&BranchTreeNode<T>, usize)> =
        nodes.iter().rev().map(|node| (node, 0)).collect();

    while let Some((node, depth)) = stack.pop() {
        match node {
            BranchTreeNode::Folder {
                path,
                label,
                branch_count,
                contains_active,
                children,
            } => {
                let expanded = is_expanded(path);
                rows.push(BranchTreeRow::Folder {
                    path: path.clone(),
                    label: label.clone(),
                    branch_count: *branch_count,
                    contains_active: *contains_active,
                    depth,
                    expanded,
                });
                if expanded {
                    stack.extend(children.iter().rev().map(|child| (child, depth + 1)));
                }
            }
            BranchTreeNode::Branch { label, item } => {
                rows.push(BranchTreeRow::Branch {
                    label: label.clone(),
                    depth,
                    item: item.clone(),
                });
            }
        }
    }

    rows
}

/// Returns all ancestor folder paths for a branch name, from outermost to
/// innermost.
///
/// For `"feature/auth/oauth"` this returns `["feature", "feature/auth"]`.
/// For `"main"` (no folder segments) this returns an empty vector.
pub fn branch_ancestor_folders(name: &str) -> Vec<String> {
    let segments: Vec<&str> = name
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() <= 1 {
        return Vec::new();
    }
    let mut ancestors = Vec::with_capacity(segments.len() - 1);
    let mut path = String::new();
    for segment in &segments[..segments.len() - 1] {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(segment);
        ancestors.push(path.clone());
    }
    ancestors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestBranch {
        name: String,
        active: bool,
    }

    fn branch(name: &str, active: bool) -> TestBranch {
        TestBranch {
            name: name.to_string(),
            active,
        }
    }

    fn tree(items: Vec<TestBranch>) -> Vec<BranchTreeNode<TestBranch>> {
        build_branch_tree(items, |item| item.name.clone(), |item| item.active)
    }

    fn shape(rows: &[BranchTreeRow<TestBranch>]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                BranchTreeRow::Folder {
                    label,
                    branch_count,
                    contains_active,
                    depth,
                    expanded,
                    ..
                } => {
                    let marker = if *expanded { "v" } else { ">" };
                    let active = if *contains_active { "*" } else { "" };
                    format!(
                        "{}{marker} {label} ({branch_count}){active}",
                        "  ".repeat(*depth)
                    )
                }
                BranchTreeRow::Branch { label, depth, item } => {
                    format!("{}{label} [{}]", "  ".repeat(*depth), item.name)
                }
            })
            .collect()
    }

    #[test]
    fn empty_input_produces_empty_tree() {
        assert!(tree(Vec::new()).is_empty());
    }

    #[test]
    fn keeps_flat_branches_at_root_in_original_order() {
        let nodes = tree(vec![branch("main", true), branch("develop", false)]);

        assert_eq!(
            nodes,
            vec![
                BranchTreeNode::Branch {
                    label: "main".to_string(),
                    item: branch("main", true),
                },
                BranchTreeNode::Branch {
                    label: "develop".to_string(),
                    item: branch("develop", false),
                },
            ]
        );
    }

    #[test]
    fn groups_single_level_folders_after_root_branches() {
        let nodes = tree(vec![
            branch("main", true),
            branch("feature/login", false),
            branch("feature/signup", false),
            branch("develop", false),
        ]);

        let rows = flatten_branch_tree(&nodes, |_| true);
        assert_eq!(
            shape(&rows),
            [
                "main [main]",
                "develop [develop]",
                "v feature (2)",
                "  login [feature/login]",
                "  signup [feature/signup]",
            ]
        );
    }

    #[test]
    fn nests_deeply_and_propagates_branch_count_and_contains_active() {
        let nodes = tree(vec![
            branch("main", false),
            branch("feature/login", false),
            branch("feature/auth/oauth", true),
            branch("feature/auth/saml", false),
            branch("fix/crash", false),
        ]);

        let rows = flatten_branch_tree(&nodes, |_| true);
        assert_eq!(
            shape(&rows),
            [
                "main [main]",
                "v feature (3)*",
                "  login [feature/login]",
                "  v auth (2)*",
                "    oauth [feature/auth/oauth]",
                "    saml [feature/auth/saml]",
                "v fix (1)",
                "  crash [fix/crash]",
            ]
        );

        let BranchTreeNode::Folder {
            path,
            branch_count,
            contains_active,
            children,
            ..
        } = &nodes[1]
        else {
            panic!("expected feature folder");
        };
        assert_eq!(path, "feature");
        assert_eq!(*branch_count, 3);
        assert!(*contains_active);
        let BranchTreeNode::Folder {
            path: nested_path,
            branch_count: nested_count,
            contains_active: nested_active,
            ..
        } = &children[1]
        else {
            panic!("expected feature/auth folder");
        };
        assert_eq!(nested_path, "feature/auth");
        assert_eq!(*nested_count, 2);
        assert!(*nested_active);
    }

    #[test]
    fn sorts_folders_alphabetically_case_insensitively() {
        let nodes = tree(vec![
            branch("main", true),
            branch("zebra/a", false),
            branch("Alpha/b", false),
            branch("beta/c", false),
            branch("feature/Zed/1", false),
            branch("feature/auth/2", false),
        ]);

        let rows = flatten_branch_tree(&nodes, |_| true);
        assert_eq!(
            shape(&rows),
            [
                "main [main]",
                "v Alpha (1)",
                "  b [Alpha/b]",
                "v beta (1)",
                "  c [beta/c]",
                "v feature (2)",
                "  v auth (1)",
                "    2 [feature/auth/2]",
                "  v Zed (1)",
                "    1 [feature/Zed/1]",
                "v zebra (1)",
                "  a [zebra/a]",
            ]
        );
    }

    #[test]
    fn handles_leading_trailing_and_double_slashes_gracefully() {
        let nodes = tree(vec![
            branch("", false),
            branch("///", false),
            branch("/single", false),
            branch("trailing/", false),
            branch("/feature//nested///leaf/", false),
        ]);

        let rows = flatten_branch_tree(&nodes, |_| true);
        assert_eq!(
            shape(&rows),
            [
                " []",
                "/// [///]",
                "/single [/single]",
                "trailing/ [trailing/]",
                "v feature (1)",
                "  v nested (1)",
                "    leaf [/feature//nested///leaf/]",
            ]
        );
    }

    #[test]
    fn coexists_when_branch_name_equals_folder_prefix() {
        let nodes = tree(vec![
            branch("feature", false),
            branch("feature/login", true),
        ]);

        let rows = flatten_branch_tree(&nodes, |_| true);
        assert_eq!(
            shape(&rows),
            [
                "feature [feature]",
                "v feature (1)*",
                "  login [feature/login]",
            ]
        );
    }

    #[test]
    fn flatten_skips_children_of_collapsed_folders() {
        let nodes = tree(vec![
            branch("main", false),
            branch("feature/login", false),
            branch("feature/auth/oauth", true),
            branch("fix/bug", false),
        ]);

        let collapsed_outer = flatten_branch_tree(&nodes, |path| path != "feature");
        assert_eq!(
            shape(&collapsed_outer),
            [
                "main [main]",
                "> feature (2)*",
                "v fix (1)",
                "  bug [fix/bug]",
            ]
        );

        let collapsed_inner = flatten_branch_tree(&nodes, |path| path != "feature/auth");
        assert_eq!(
            shape(&collapsed_inner),
            [
                "main [main]",
                "v feature (2)*",
                "  login [feature/login]",
                "  > auth (1)*",
                "v fix (1)",
                "  bug [fix/bug]",
            ]
        );
    }

    #[test]
    fn computes_ancestor_folders_for_branch_names() {
        assert!(branch_ancestor_folders("main").is_empty());
        assert!(branch_ancestor_folders("").is_empty());
        assert!(branch_ancestor_folders("///").is_empty());
        assert!(branch_ancestor_folders("/main/").is_empty());
        assert_eq!(branch_ancestor_folders("feature/login"), ["feature"]);
        assert_eq!(
            branch_ancestor_folders("feature/auth/oauth"),
            ["feature", "feature/auth"]
        );
        assert_eq!(
            branch_ancestor_folders("/feature//auth/oauth/"),
            ["feature", "feature/auth"]
        );
    }
}
