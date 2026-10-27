//! Language-isolated declarations and differential reverse references.
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
};

/// Supported index partitions. Only PHP has a parser currently.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum LanguageId {
    /// PHP source.
    Php,
    /// Go partition reserved for a future parser.
    Go,
    /// JavaScript/TypeScript partition reserved for a future parser.
    JavaScript,
}
/// Qualified symbol identity inside one language partition.
pub type SymbolFqn = String;
/// Per-file authoritative declaration table.
pub type DeclarationsByFile = HashMap<PathBuf, Vec<SymbolDeclaration>>;
/// Reverse edges, with source locations deduplicated.
pub type ReferencesBySymbol = HashMap<SymbolFqn, HashSet<ReferenceLocation>>;
/// Declaration category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SymbolKind {
    /// Class, interface or trait.
    Class,
    /// Global function.
    Function,
    /// Instance or static method.
    Method,
    /// Instance or static property.
    Property,
    /// Global or class constant.
    Constant,
    /// Local variable or parameter.
    Variable,
}
/// Access control.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Visibility {
    /// Public API.
    Public,
    /// Subclass API.
    Protected,
    /// Declaring class only.
    Private,
}
/// Exact UTF-8 name coordinates and display metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolDeclaration {
    /// Fully qualified identity.
    pub fqn: SymbolFqn,
    /// Short source name.
    pub name: String,
    /// Declaration category.
    pub kind: SymbolKind,
    /// Native source path.
    pub path: PathBuf,
    /// Name bytes in the original file.
    pub range: Range<usize>,
    /// Zero-based line.
    pub line: u32,
    /// Zero-based UTF-8 column.
    pub column: u32,
    /// Declaring class, if any.
    pub owner: Option<String>,
    /// Direct parent classes/interfaces, in PHP lookup order (class declarations only).
    #[serde(default)]
    pub base_types: Vec<String>,
    /// Lexical scope bytes.
    pub scope: Range<usize>,
    /// PHP access modifier.
    pub visibility: Visibility,
    /// Static member flag.
    pub is_static: bool,
    /// Inferred or declared class type.
    pub type_name: Option<String>,
    /// Source signature.
    pub signature: String,
    /// Existing source doc comment.
    pub documentation: String,
    /// Number of resolved references.
    pub usages_count: usize,
}
impl SymbolDeclaration {
    /// Methods/properties without indexed references, and unreferenced private constants.
    /// Dynamic runtime calls are outside this static inspection.
    #[must_use]
    pub fn is_unused(&self) -> bool {
        self.usages_count == 0
            && (matches!(self.kind, SymbolKind::Method | SymbolKind::Property)
                || (self.kind == SymbolKind::Constant && self.visibility == Visibility::Private))
            && !self.name.starts_with("__")
    }
}
/// One deduplicated use; preview is captured off the UI thread.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ReferenceLocation {
    /// Source file.
    pub path: PathBuf,
    /// UTF-8 name start.
    pub byte: usize,
    /// Name length in bytes.
    pub length: usize,
    /// Zero-based line.
    pub line: u32,
    /// Zero-based byte column.
    pub column: u32,
    /// Original line content.
    pub preview: String,
}
/// Parsed per-file facts, retained so linking never needs another parse.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileSymbols {
    /// Source declarations.
    pub declarations: Vec<SymbolDeclaration>,
    /// Candidate names in PHP resolution order and their source locations.
    pub references: Vec<(Vec<String>, ReferenceLocation)>,
    /// Namespace/import contexts, ordered by source position; no AST is retained.
    pub name_scopes: Vec<NameScope>,
}
/// Source namespace and class import aliases for receiver completion.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NameScope {
    /// Applicable UTF-8 bytes.
    pub range: Range<usize>,
    /// Unprefixed namespace.
    pub namespace: String,
    /// Lowercase alias to qualified class name.
    pub aliases: HashMap<String, String>,
}
/// One language's index. Replacing a file removes exactly its previous edges.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolIndex {
    /// Partition identity.
    pub language: LanguageId,
    /// Declaration store.
    pub declarations_by_file: DeclarationsByFile,
    /// Resolved reverse edges.
    pub references_by_symbol: ReferencesBySymbol,
    files: HashMap<PathBuf, FileSymbols>,
    candidates: HashMap<String, HashSet<PathBuf>>,
    #[serde(default)]
    dependencies_by_file: HashMap<PathBuf, HashSet<String>>,
    #[serde(default)]
    resolved_by_file: HashMap<PathBuf, Vec<(String, ReferenceLocation)>>,
}
impl SymbolIndex {
    /// Creates an empty language partition.
    #[must_use]
    pub fn new(language: LanguageId) -> Self {
        Self {
            language,
            declarations_by_file: HashMap::new(),
            references_by_symbol: HashMap::new(),
            files: HashMap::new(),
            candidates: HashMap::new(),
            dependencies_by_file: HashMap::new(),
            resolved_by_file: HashMap::new(),
        }
    }
    /// Retrieves immutable parsed facts for restoring a disk overlay.
    #[must_use]
    pub fn file_symbols(&self, path: &Path) -> Option<&FileSymbols> {
        self.files.get(path)
    }
    /// Iterates declarations without filesystem access.
    pub fn declarations(&self) -> impl Iterator<Item = &SymbolDeclaration> {
        self.declarations_by_file.values().flatten()
    }
    /// Resolves an exact canonical identity (PHP callable identities are normalized by the parser).
    #[must_use]
    pub fn declaration(&self, fqn: &str) -> Option<&SymbolDeclaration> {
        self.declarations().find(|d| d.fqn == fqn)
    }
    /// Class lookup order, including missing parents so later declarations can relink callers.
    /// Cycles in malformed/incomplete projects are visited only once.
    #[must_use]
    pub fn class_lineage(&self, class: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut pending = vec![class.to_string()];
        let mut visited = HashSet::new();
        while let Some(name) = pending.pop() {
            if !visited.insert(name.clone()) {
                continue;
            }
            if let Some(declaration) = self.declaration(&name) {
                pending.extend(declaration.base_types.iter().rev().cloned());
            }
            result.push(name);
        }
        result
    }
    /// Resolves a direct symbol or the actual declaration of an inherited member.
    #[must_use]
    pub fn resolve(&self, fqn: &str) -> Option<&SymbolDeclaration> {
        if let Some(declaration) = self.declaration(fqn) {
            return Some(declaration);
        }
        let (owner, member) = fqn.rsplit_once("::")?;
        self.class_lineage(owner)
            .into_iter()
            .skip(1)
            .find_map(|parent| {
                self.declaration(&format!("{parent}::{member}"))
                    .filter(|d| d.visibility != Visibility::Private)
            })
    }
    fn reference_dependencies(&self, candidates: &[String]) -> HashSet<String> {
        let mut result = HashSet::new();
        for name in candidates {
            result.insert(name.clone());
            if let Some((owner, _)) = name.rsplit_once("::") {
                result.extend(self.class_lineage(owner));
            }
        }
        result
    }
    /// Removes a file and its edges. Dependent facts are relinked, never reparsed.
    pub fn remove(&mut self, path: &Path) {
        self.replace(path.to_path_buf(), FileSymbols::default());
        self.files.remove(path);
        self.declarations_by_file.remove(path);
        self.dependencies_by_file.remove(path);
        self.resolved_by_file.remove(path);
    }
    /// Atomically replaces one file. Incoming hierarchy dependencies are relinked from saved facts.
    pub fn replace(&mut self, path: PathBuf, facts: FileSymbols) {
        let mut touched: HashSet<_> = self
            .declarations_by_file
            .get(&path)
            .into_iter()
            .flatten()
            .chain(&facts.declarations)
            .map(|d| d.fqn.clone())
            .collect();
        let mut affected_files = HashSet::from([path.clone()]);
        for name in &touched {
            if let Some(files) = self.candidates.get(name) {
                affected_files.extend(files.iter().cloned());
            }
        }
        // Remove the actual old targets, which can differ from the caller's inherited member key.
        for file in &affected_files {
            if let Some(edges) = self.resolved_by_file.remove(file) {
                for (target, location) in edges {
                    if let Some(bucket) = self.references_by_symbol.get_mut(&target) {
                        bucket.remove(&location);
                    }
                    touched.insert(target);
                }
            }
            if let Some(dependencies) = self.dependencies_by_file.remove(file) {
                for name in dependencies {
                    if let Some(files) = self.candidates.get_mut(&name) {
                        files.remove(file);
                    }
                }
            }
        }
        self.declarations_by_file
            .insert(path.clone(), facts.declarations.clone());
        self.files.insert(path, facts);
        for file in affected_files {
            let mut dependencies = HashSet::new();
            let mut edges = Vec::new();
            if let Some(facts) = self.files.get(&file) {
                for (names, location) in &facts.references {
                    dependencies.extend(self.reference_dependencies(names));
                    if let Some(declaration) = names.iter().find_map(|name| self.resolve(name)) {
                        edges.push((declaration.fqn.clone(), location.clone()));
                    }
                }
            }
            for name in &dependencies {
                self.candidates
                    .entry(name.clone())
                    .or_default()
                    .insert(file.clone());
            }
            for (target, location) in &edges {
                self.references_by_symbol
                    .entry(target.clone())
                    .or_default()
                    .insert(location.clone());
                touched.insert(target.clone());
            }
            self.dependencies_by_file.insert(file.clone(), dependencies);
            self.resolved_by_file.insert(file, edges);
        }
        self.candidates.retain(|_, files| !files.is_empty());
        self.references_by_symbol.retain(|_, refs| !refs.is_empty());
        for d in self
            .declarations_by_file
            .values_mut()
            .flatten()
            .filter(|d| touched.contains(&d.fqn))
        {
            d.usages_count = self
                .references_by_symbol
                .get(&d.fqn)
                .map_or(0, HashSet::len);
        }
    }
    /// Exact declaration/reference under a UTF-8 byte offset.
    #[must_use]
    pub fn at(&self, path: &Path, byte: usize) -> Option<&SymbolDeclaration> {
        if let Some(d) = self
            .declarations_by_file
            .get(path)?
            .iter()
            .find(|d| d.range.contains(&byte))
        {
            return Some(d);
        }
        let facts = self.files.get(path)?;
        let (names, _) = facts
            .references
            .iter()
            .find(|(_, r)| (r.byte..r.byte + r.length).contains(&byte))?;
        names.iter().find_map(|name| self.resolve(name))
    }
}
