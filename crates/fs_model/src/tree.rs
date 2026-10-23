//! Lazy project tree and shared ignore policy.
use crate::FsError;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Basic syntax/icon family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Rust source.
    Rust,
    /// PHP source.
    Php,
    /// JavaScript or TypeScript.
    Script,
    /// TOML or JSON.
    Config,
    /// Markdown.
    Markdown,
    /// Other file.
    Generic,
}
impl FileKind {
    /// Classifies an extension without opening the file.
    #[must_use]
    pub fn of(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "rs" => Self::Rust,
            "php" => Self::Php,
            "js" | "jsx" | "ts" | "tsx" => Self::Script,
            "toml" | "json" => Self::Config,
            "md" | "markdown" => Self::Markdown,
            _ => Self::Generic,
        }
    }
    /// Compact UI marker.
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Rust => "RS",
            Self::Php => "PHP",
            Self::Script => "JS/TS",
            Self::Config => "{}",
            Self::Markdown => "MD",
            Self::Generic => "·",
        }
    }
}
/// File identity and display family.
#[derive(Debug, Clone)]
pub struct FileNode {
    /// Absolute native path.
    pub path: PathBuf,
    /// Extension-derived family.
    pub kind: FileKind,
}
/// One directory; `None` means children have never been requested.
#[derive(Debug, Clone)]
pub struct DirNode {
    /// Absolute native path.
    pub path: PathBuf,
    /// Sorted immediate children, loaded on demand.
    pub children: Option<Vec<Node>>,
}
/// Immediate child in a directory.
#[derive(Debug, Clone)]
pub enum Node {
    /// Lazy directory reference.
    Dir(PathBuf),
    /// File leaf.
    File(FileNode),
}
impl Node {
    /// Native absolute path.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Dir(p) => p,
            Self::File(f) => &f.path,
        }
    }
}
/// Worker-owned lazy tree. Index scans do not populate its children.
#[derive(Debug, Clone)]
pub struct ProjectTree {
    /// Canonical project root.
    pub root_path: PathBuf,
    /// Only directories discovered through expansion.
    pub directories: BTreeMap<PathBuf, DirNode>,
}
impl ProjectTree {
    /// Resolves an existing directory; does not scan children.
    /// # Errors
    /// Returns I/O errors or rejects a non-directory root.
    pub fn new(root: &Path) -> Result<Self, FsError> {
        let root_path = root.canonicalize()?;
        if !root_path.is_dir() {
            return Err(FsError::NotDirectory(root_path));
        }
        let directories = BTreeMap::from([(
            root_path.clone(),
            DirNode {
                path: root_path.clone(),
                children: None,
            },
        )]);
        Ok(Self {
            root_path,
            directories,
        })
    }
    /// Loads one level, applying ignore files from this directory and ancestors.
    /// # Errors
    /// Returns traversal errors or rejects paths outside the project.
    pub fn load(&mut self, path: &Path) -> Result<(), FsError> {
        let canonical = path.canonicalize()?;
        let path = canonical.as_path();
        if !path.starts_with(&self.root_path) {
            return Err(FsError::OutsideProject);
        }
        let mut nodes = Vec::new();
        for entry in walker(path, Some(1)).build() {
            let entry = entry?;
            if entry.depth() == 0 {
                continue;
            }
            let p = entry.path().to_path_buf();
            if entry.file_type().is_some_and(|t| t.is_dir()) {
                self.directories
                    .entry(p.clone())
                    .or_insert_with(|| DirNode {
                        path: p.clone(),
                        children: None,
                    });
                nodes.push(Node::Dir(p));
            } else if entry.file_type().is_some_and(|t| t.is_file()) {
                nodes.push(Node::File(FileNode {
                    kind: FileKind::of(&p),
                    path: p,
                }));
            }
        }
        nodes.sort_by(|a, b| {
            (!matches!(a, Node::Dir(_)), a.path().file_name())
                .cmp(&(!matches!(b, Node::Dir(_)), b.path().file_name()))
        });
        self.directories.insert(
            path.to_path_buf(),
            DirNode {
                path: path.to_path_buf(),
                children: Some(nodes),
            },
        );
        Ok(())
    }
}
pub(crate) fn service_path(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some(".git" | "target" | "node_modules" | "vendor")
        )
    })
}
pub(crate) fn walker(path: &Path, depth: Option<usize>) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(path);
    builder
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .max_depth(depth)
        .filter_entry(|e| e.depth() == 0 || !service_path(Path::new(e.file_name())));
    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lazy_sorted_and_ignored() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        for name in ["z", "a", "target", "node_modules", "vendor", ".git"] {
            std::fs::create_dir(dir.path().join(name))?;
        }
        for name in ["b.rs", "a.md", "hidden.log"] {
            std::fs::write(dir.path().join(name), "")?;
        }
        std::fs::write(dir.path().join(".gitignore"), "*.log\n")?;
        std::fs::write(dir.path().join("a/nested.rs"), "")?;
        let mut tree = ProjectTree::new(dir.path())?;
        let root = tree.root_path.clone();
        assert!(tree.directories[&root].children.is_none());
        assert!(matches!(
            tree.load(&root.join("..")),
            Err(FsError::OutsideProject)
        ));
        tree.load(&root)?;
        let children = tree.directories[&root]
            .children
            .as_ref()
            .ok_or("missing children")?;
        let names: Vec<_> = children
            .iter()
            .filter_map(|n| n.path().file_name().and_then(|s| s.to_str()))
            .collect();
        assert_eq!(names, ["a", "z", ".gitignore", "a.md", "b.rs"]);
        assert!(tree.directories[&root.join("a")].children.is_none());
        tree.load(&root.join("a"))?;
        assert_eq!(
            tree.directories[&root.join("a")]
                .children
                .as_ref()
                .map(Vec::len),
            Some(1)
        );
        Ok(())
    }
}
