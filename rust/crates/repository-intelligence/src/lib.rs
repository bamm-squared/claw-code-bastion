//! Deterministic, trusted-side structural repository intelligence.
//!
//! This crate parses source bytes locally and produces content-addressed fact
//! packs plus one queryable graph. It never executes repository code and does
//! not expose its cache or graph to the model or worker.

#![allow(clippy::manual_let_else)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::single_match_else)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::unnested_or_patterns)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;
use tree_sitter::{Language as TsLanguage, Node, Parser};
use walkdir::WalkDir;

pub const SCHEMA_VERSION: u32 = 1;
pub const EXTRACTOR_VERSION: &str = "tree-sitter-structural-1";
pub const GRAMMAR_VERSION: &str = "tree-sitter-grammars-0.23";
pub const DEFAULT_MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_REFERENCES_PER_FILE: usize = 8192;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_SYMBOLS_PER_FILE: usize = 4096;
const MAX_IMPORTS_PER_FILE: usize = 2048;
const MAX_STRUCTURAL_FACTS_PER_FILE: usize = 4096;
const MAX_DIAGNOSTICS_PER_FILE: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ContentIdentity(pub [u8; 32]);

impl ContentIdentity {
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }
}

impl Display for ContentIdentity {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct AnalysisIdentity(pub [u8; 32]);

impl AnalysisIdentity {
    #[must_use]
    pub fn new(content: ContentIdentity, language: Language, config: &AnalysisConfig) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(content.0);
        hasher.update(EXTRACTOR_VERSION.as_bytes());
        hasher.update(GRAMMAR_VERSION.as_bytes());
        hasher.update(language.as_str().as_bytes());
        hasher.update(SCHEMA_VERSION.to_le_bytes());
        hasher.update(config.identity_bytes());
        Self(hasher.finalize().into())
    }
}

impl Display for AnalysisIdentity {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub max_source_bytes: u64,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            max_source_bytes: DEFAULT_MAX_SOURCE_BYTES,
        }
    }
}

impl AnalysisConfig {
    fn identity_bytes(&self) -> Vec<u8> {
        self.max_source_bytes.to_le_bytes().to_vec()
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum Language {
    Rust,
    Python,
    Java,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    C,
    Cpp,
    CSharp,
    Html,
    Css,
}

impl Language {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::Java => "Java",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TSX",
            Self::Go => "Go",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::CSharp => "C#",
            Self::Html => "HTML",
            Self::Css => "CSS",
        }
    }
}

#[must_use]
pub fn language_for_path(path: &Path) -> Option<Language> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => Language::Rust,
        "py" => Language::Python,
        "java" => Language::Java,
        "js" | "mjs" | "cjs" => Language::JavaScript,
        "ts" => Language::TypeScript,
        "tsx" | "jsx" => Language::Tsx,
        "go" => Language::Go,
        "c" | "h" => Language::C,
        "cc" | "cpp" | "cxx" | "hpp" => Language::Cpp,
        "cs" => Language::CSharp,
        "html" | "htm" => Language::Html,
        "css" => Language::Css,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Evidence {
    Exact,
    Derived,
    Heuristic,
    Advisory,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: String,
    pub analyzer: String,
    pub analyzer_version: String,
    pub analysis_identity: AnalysisIdentity,
    pub evidence: Evidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceRange {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_row: usize,
    pub start_column: usize,
    pub end_row: usize,
    pub end_column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SymbolFact {
    pub kind: String,
    pub name: String,
    pub qualified_name: String,
    pub parent: Option<String>,
    pub visibility: Option<String>,
    pub range: SourceRange,
    pub declaration: String,
    pub attributes: Vec<String>,
    pub doc_comment: Option<String>,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImportFact {
    pub declaration: String,
    pub target: String,
    pub resolution: String,
    pub range: SourceRange,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReferenceFact {
    pub name: String,
    pub resolved_symbol: Option<String>,
    pub range: SourceRange,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StructuralFact {
    pub kind: String,
    pub value: String,
    pub range: Option<SourceRange>,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ParseDiagnostics {
    pub contains_error_nodes: bool,
    pub error_count: usize,
    pub affected_ranges: Vec<SourceRange>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtractorProvenance {
    pub source: String,
    pub extractor: String,
    pub extractor_version: String,
    pub grammar: String,
    pub schema_version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileFactPack {
    pub schema_version: u32,
    pub relative_path: String,
    pub language: Language,
    pub content_identity: ContentIdentity,
    pub analysis_identity: AnalysisIdentity,
    pub source_size: u64,
    pub symbols: Vec<SymbolFact>,
    pub imports: Vec<ImportFact>,
    pub references: Vec<ReferenceFact>,
    pub structural_facts: Vec<StructuralFact>,
    pub parse_diagnostics: ParseDiagnostics,
    pub extractor_provenance: ExtractorProvenance,
    pub facts_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectDependency {
    pub name: String,
    pub requirement: String,
    pub kind: String,
    pub local_manifest: Option<String>,
    pub resolved_package: Option<String>,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectFact {
    pub manifest_path: String,
    pub ecosystem: String,
    pub package_name: String,
    pub package_root: String,
    pub workspace_root: Option<String>,
    pub version: Option<String>,
    pub dependencies: Vec<ProjectDependency>,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SkipReason {
    Unsupported,
    Oversize,
    Binary,
    SpecialFile,
    InvalidPath,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkippedFile {
    pub relative_path: String,
    pub reason: SkipReason,
}

fn ts_language(language: Language) -> TsLanguage {
    match language {
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::Python => tree_sitter_python::LANGUAGE.into(),
        Language::Java => tree_sitter_java::LANGUAGE.into(),
        Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Language::Go => tree_sitter_go::LANGUAGE.into(),
        Language::C => tree_sitter_c::LANGUAGE.into(),
        Language::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        Language::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
        Language::Html => tree_sitter_html::LANGUAGE.into(),
        Language::Css => tree_sitter_css::LANGUAGE.into(),
    }
}

fn range(node: Node<'_>) -> SourceRange {
    let s = node.start_position();
    let e = node.end_position();
    SourceRange {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_row: s.row,
        start_column: s.column,
        end_row: e.row,
        end_column: e.column,
    }
}

fn node_text(node: Node<'_>, source: &[u8]) -> String {
    String::from_utf8_lossy(&source[node.byte_range()])
        .chars()
        .take(512)
        .collect()
}

fn collect_nodes<'tree>(node: Node<'tree>, nodes: &mut Vec<Node<'tree>>) {
    nodes.push(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_nodes(child, nodes);
    }
}

fn name_node(node: Node<'_>) -> Option<Node<'_>> {
    ["name", "declarator", "field_identifier", "type"]
        .iter()
        .find_map(|field| node.child_by_field_name(field))
}

fn declaration_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let candidate = name_node(node)?;
    if matches!(
        candidate.kind(),
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "property_identifier"
            | "shorthand_property_identifier_pattern"
    ) {
        return Some(node_text(candidate, source));
    }
    let mut cursor = candidate.walk();
    for child in candidate.children(&mut cursor) {
        if let Some(name) = declaration_name(child, source) {
            return Some(name);
        }
    }
    Some(node_text(candidate, source))
}

fn symbol_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function_item"
        | "function_definition"
        | "function_declaration"
        | "method_declaration"
        | "method_definition"
        | "function"
        | "arrow_function"
        | "func_declaration" => "function",
        "struct_item" | "struct_specifier" => "struct",
        "enum_item" | "enum_specifier" => "enum",
        "class_declaration" | "class_definition" | "class_specifier" => "class",
        "interface_declaration" | "interface_definition" => "interface",
        "trait_item" => "trait",
        "type_item" | "type_alias_declaration" | "type_definition" => "type_alias",
        "impl_item" | "namespace_definition" | "module" | "namespace_declaration" => "module",
        "const_item" | "static_item" | "lexical_declaration" => "constant",
        "field_declaration" | "field_definition" | "property_declaration" => "field",
        "macro_definition" => "macro",
        _ => return None,
    })
}

fn import_kind(language: Language, kind: &str) -> bool {
    matches!(
        (language, kind),
        (
            Language::Rust,
            "use_declaration" | "mod_item" | "extern_crate_declaration"
        ) | (
            Language::Python,
            "import_statement" | "import_from_statement"
        ) | (Language::Java, "import_declaration")
            | (
                Language::JavaScript | Language::TypeScript | Language::Tsx,
                "import_statement" | "export_statement"
            )
            | (Language::Go, "import_declaration")
            | (Language::C | Language::Cpp, "preproc_include")
            | (Language::CSharp, "using_directive")
            | (Language::Css, "import_statement")
    )
}

fn rust_reference_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "async"
            | "await"
            | "const"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
    )
}

#[must_use]
pub fn extract_file(
    relative_path: &str,
    language: Language,
    source: &[u8],
    config: &AnalysisConfig,
) -> FileFactPack {
    let content = ContentIdentity::from_bytes(source);
    let analysis = AnalysisIdentity::new(content, language, config);
    let provenance = |evidence| Provenance {
        source: "TreeSitter".into(),
        analyzer: "repository-intelligence".into(),
        analyzer_version: EXTRACTOR_VERSION.into(),
        analysis_identity: analysis,
        evidence,
    };
    let mut parser = Parser::new();
    parser
        .set_language(&ts_language(language))
        .expect("bundled grammar must load");
    let tree = parser.parse(source, None).expect("parser returns a tree");
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut references = Vec::new();
    let mut structural = Vec::new();
    let mut facts_truncated = false;
    let mut stack = vec![(tree.root_node(), None::<String>)];
    while let Some((node, parent)) = stack.pop() {
        if let Some(kind) = symbol_kind(node.kind()) {
            if let Some(name) = declaration_name(node, source) {
                let qualified = parent
                    .as_ref()
                    .map_or_else(|| name.clone(), |p| format!("{p}::{name}"));
                if symbols.len() < MAX_SYMBOLS_PER_FILE {
                    symbols.push(SymbolFact {
                        kind: kind.into(),
                        name,
                        qualified_name: qualified.clone(),
                        parent: parent.clone(),
                        visibility: None,
                        range: range(node),
                        declaration: node_text(node, source),
                        attributes: Vec::new(),
                        doc_comment: None,
                        provenance: provenance(Evidence::Exact),
                    });
                } else {
                    facts_truncated = true;
                }
            }
        }
        if import_kind(language, node.kind()) {
            let text = node_text(node, source);
            if imports.len() < MAX_IMPORTS_PER_FILE {
                imports.push(ImportFact {
                    declaration: text.clone(),
                    target: text,
                    resolution: "UNRESOLVED".into(),
                    range: range(node),
                    provenance: provenance(Evidence::Exact),
                });
            } else {
                facts_truncated = true;
            }
        }
        if language == Language::Rust && node.kind() == "identifier" {
            let name = node_text(node, source);
            let is_declaration = symbols.iter().any(|symbol| {
                symbol.range.start_byte <= node.start_byte()
                    && node.end_byte() <= symbol.range.end_byte
                    && symbol.name == name
            });
            if !is_declaration && !rust_reference_keyword(&name) {
                if references.len() < MAX_REFERENCES_PER_FILE {
                    references.push(ReferenceFact {
                        name,
                        resolved_symbol: None,
                        range: range(node),
                        provenance: provenance(Evidence::Derived),
                    });
                } else {
                    facts_truncated = true;
                }
            }
        }
        if matches!(language, Language::Html | Language::Css)
            && matches!(
                node.kind(),
                "attribute"
                    | "class_selector"
                    | "id_selector"
                    | "tag_name"
                    | "custom_property_name"
                    | "keyframes_statement"
                    | "import_statement"
            )
        {
            if structural.len() < MAX_STRUCTURAL_FACTS_PER_FILE {
                structural.push(StructuralFact {
                    kind: node.kind().into(),
                    value: node_text(node, source),
                    range: Some(range(node)),
                    provenance: provenance(Evidence::Exact),
                });
            } else {
                facts_truncated = true;
            }
        }
        let next_parent = symbol_kind(node.kind())
            .and_then(|_| declaration_name(node, source))
            .or(parent);
        let mut cursor = node.walk();
        let mut children: Vec<_> = node.children(&mut cursor).collect();
        children.reverse();
        for child in children {
            stack.push((child, next_parent.clone()));
        }
    }
    symbols.sort_by(|a, b| {
        (a.range.start_byte, &a.qualified_name).cmp(&(b.range.start_byte, &b.qualified_name))
    });
    imports.sort_by_key(|a| a.range.start_byte);
    references.sort_by_key(|a| a.range.start_byte);
    structural.sort_by(|a, b| {
        a.range
            .as_ref()
            .map_or(0, |r| r.start_byte)
            .cmp(&b.range.as_ref().map_or(0, |r| r.start_byte))
    });
    let mut diagnostics = Vec::new();
    let mut nodes = Vec::new();
    collect_nodes(tree.root_node(), &mut nodes);
    for node in nodes {
        if node.is_error() || node.is_missing() {
            if diagnostics.len() < MAX_DIAGNOSTICS_PER_FILE {
                diagnostics.push(range(node));
            } else {
                facts_truncated = true;
            }
        }
    }
    FileFactPack {
        schema_version: SCHEMA_VERSION,
        relative_path: relative_path.into(),
        language,
        content_identity: content,
        analysis_identity: analysis,
        source_size: source.len() as u64,
        symbols,
        imports,
        references,
        structural_facts: structural,
        parse_diagnostics: ParseDiagnostics {
            contains_error_nodes: !diagnostics.is_empty(),
            error_count: diagnostics.len(),
            affected_ranges: diagnostics,
        },
        extractor_provenance: ExtractorProvenance {
            source: "TreeSitter".into(),
            extractor: "repository-intelligence".into(),
            extractor_version: EXTRACTOR_VERSION.into(),
            grammar: language.as_str().into(),
            schema_version: SCHEMA_VERSION,
        },
        facts_truncated,
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildStats {
    pub supported_files: usize,
    pub parsed_files: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub files_skipped: usize,
    pub nodes: usize,
    pub edges: usize,
    pub build_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildOutput {
    pub graph: RepositoryGraph,
    pub stats: BuildStats,
    pub skipped: Vec<SkippedFile>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    Repository,
    File,
    Symbol,
    ImportTarget,
    Project,
    Package,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    pub provenance: Option<Provenance>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub kind: String,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepositoryGraph {
    pub nodes: BTreeMap<String, GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub facts: BTreeMap<String, FileFactPack>,
    pub project_facts: BTreeMap<String, ProjectFact>,
}

impl RepositoryGraph {
    fn add_pack(&mut self, pack: FileFactPack) {
        let file_id = format!("file:{}", pack.relative_path);
        self.nodes.insert(
            file_id.clone(),
            GraphNode {
                id: file_id.clone(),
                kind: NodeKind::File,
                label: pack.relative_path.clone(),
                provenance: None,
            },
        );
        self.edges.push(GraphEdge {
            source: "repository".into(),
            target: file_id.clone(),
            kind: "CONTAINS".into(),
            provenance: pack.symbols.first().map_or_else(
                || pack_provenance(&pack),
                |symbol| symbol.provenance.clone(),
            ),
        });
        for symbol in &pack.symbols {
            let id = format!("symbol:{}:{}", pack.relative_path, symbol.qualified_name);
            self.nodes.insert(
                id.clone(),
                GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Symbol,
                    label: symbol.qualified_name.clone(),
                    provenance: Some(symbol.provenance.clone()),
                },
            );
            self.edges.push(GraphEdge {
                source: file_id.clone(),
                target: id,
                kind: "DEFINES".into(),
                provenance: symbol.provenance.clone(),
            });
        }
        for import in &pack.imports {
            let id = format!("import:{}:{}", pack.relative_path, import.target);
            self.nodes.entry(id.clone()).or_insert(GraphNode {
                id: id.clone(),
                kind: NodeKind::ImportTarget,
                label: import.target.clone(),
                provenance: Some(import.provenance.clone()),
            });
            self.edges.push(GraphEdge {
                source: file_id.clone(),
                target: id,
                kind: "IMPORTS".into(),
                provenance: import.provenance.clone(),
            });
        }
        self.facts.insert(pack.relative_path.clone(), pack);
    }
    fn remove_file(&mut self, path: &str) {
        let file_id = format!("file:{path}");
        self.nodes.remove(&file_id);
        self.facts.remove(path);
        let removed: BTreeSet<String> = self
            .edges
            .iter()
            .filter(|edge| edge.source == file_id)
            .map(|edge| edge.target.clone())
            .collect();
        self.edges
            .retain(|edge| edge.source != file_id && !removed.contains(&edge.target));
        for node in removed {
            self.nodes.remove(&node);
        }
    }
    fn finish(&mut self) {
        self.edges
            .sort_by(|a, b| (&a.source, &a.kind, &a.target).cmp(&(&b.source, &b.kind, &b.target)));
        self.edges.dedup();
    }

    fn enrich_semantics(&mut self) {
        self.edges.retain(|edge| edge.kind != "REFERENCES");
        let mut definitions = BTreeMap::new();
        for (path, pack) in &self.facts {
            for symbol in &pack.symbols {
                definitions.insert(
                    (path.clone(), symbol.name.clone()),
                    format!("symbol:{path}:{}", symbol.qualified_name),
                );
            }
        }
        let paths: BTreeSet<String> = self.facts.keys().cloned().collect();
        let mut additions = Vec::new();
        for (path, pack) in &mut self.facts {
            if pack.language != Language::Rust {
                continue;
            }
            for reference in &mut pack.references {
                reference.resolved_symbol = None;
            }
            let mut imported = BTreeMap::new();
            for import in &mut pack.imports {
                import.resolution = "UNRESOLVED".into();
                let Some((name, module)) = rust_import_parts(&import.target) else {
                    continue;
                };
                let Some(target_path) = rust_module_path(path, &module, &paths) else {
                    continue;
                };
                let Some(symbol_id) = definitions.get(&(target_path, name.clone())) else {
                    continue;
                };
                import.resolution.clone_from(symbol_id);
                imported.insert(name, symbol_id.clone());
            }
            for reference in &mut pack.references {
                let symbol_id = imported.get(&reference.name).cloned().or_else(|| {
                    definitions
                        .get(&(path.clone(), reference.name.clone()))
                        .cloned()
                });
                let Some(symbol_id) = symbol_id else {
                    continue;
                };
                reference.resolved_symbol = Some(symbol_id.clone());
                additions.push(GraphEdge {
                    source: format!("file:{path}"),
                    target: symbol_id,
                    kind: "REFERENCES".into(),
                    provenance: semantic_provenance(reference.provenance.clone()),
                });
            }
        }
        self.edges.extend(additions);
    }

    fn add_project_fact(&mut self, fact: ProjectFact) {
        let project_id = format!("project:{}", fact.manifest_path);
        let package_id = format!("package:{}:{}", fact.manifest_path, fact.package_name);
        self.nodes.insert(
            project_id.clone(),
            GraphNode {
                id: project_id.clone(),
                kind: NodeKind::Project,
                label: fact.manifest_path.clone(),
                provenance: Some(fact.provenance.clone()),
            },
        );
        self.nodes.insert(
            package_id.clone(),
            GraphNode {
                id: package_id.clone(),
                kind: NodeKind::Package,
                label: fact.package_name.clone(),
                provenance: Some(fact.provenance.clone()),
            },
        );
        self.edges.push(GraphEdge {
            source: "repository".into(),
            target: project_id.clone(),
            kind: "CONTAINS".into(),
            provenance: fact.provenance.clone(),
        });
        if let Some(workspace_root) = &fact.workspace_root {
            let workspace_id = format!("project:{workspace_root}");
            self.nodes.entry(workspace_id.clone()).or_insert(GraphNode {
                id: workspace_id.clone(),
                kind: NodeKind::Project,
                label: workspace_root.clone(),
                provenance: Some(fact.provenance.clone()),
            });
            self.edges.push(GraphEdge {
                source: workspace_id,
                target: package_id.clone(),
                kind: "CONTAINS".into(),
                provenance: fact.provenance.clone(),
            });
        }
        self.edges.push(GraphEdge {
            source: project_id,
            target: package_id.clone(),
            kind: "CONTAINS".into(),
            provenance: fact.provenance.clone(),
        });
        for dependency in &fact.dependencies {
            let target = dependency.resolved_package.clone().unwrap_or_else(|| {
                format!("package:external:{}:{}", fact.ecosystem, dependency.name)
            });
            self.nodes.entry(target.clone()).or_insert(GraphNode {
                id: target.clone(),
                kind: NodeKind::Package,
                label: dependency.name.clone(),
                provenance: Some(dependency.provenance.clone()),
            });
            self.edges.push(GraphEdge {
                source: package_id.clone(),
                target,
                kind: "DEPENDS_ON".into(),
                provenance: dependency.provenance.clone(),
            });
        }
        for path in self.facts.keys() {
            if path == &fact.manifest_path
                || Path::new(path)
                    .parent()
                    .is_some_and(|parent| parent == Path::new(&fact.package_root))
                || path.starts_with(&format!("{}/", fact.package_root))
            {
                self.edges.push(GraphEdge {
                    source: format!("file:{path}"),
                    target: package_id.clone(),
                    kind: "BELONGS_TO".into(),
                    provenance: fact.provenance.clone(),
                });
            }
        }
        self.project_facts.insert(fact.manifest_path.clone(), fact);
    }

    fn clear_project_metadata(&mut self) {
        self.project_facts.clear();
        let project_nodes: BTreeSet<String> = self
            .nodes
            .values()
            .filter(|node| matches!(node.kind, NodeKind::Project | NodeKind::Package))
            .map(|node| node.id.clone())
            .collect();
        self.nodes.retain(|id, _| !project_nodes.contains(id));
        self.edges.retain(|edge| {
            !project_nodes.contains(&edge.source)
                && !project_nodes.contains(&edge.target)
                && edge.kind != "DEPENDS_ON"
                && edge.kind != "BELONGS_TO"
        });
    }
    #[must_use]
    pub fn file_facts(&self, path: &str) -> Option<&FileFactPack> {
        self.facts.get(path)
    }
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.get(id)
    }
    #[must_use]
    pub fn outgoing(&self, id: &str) -> Vec<&GraphEdge> {
        self.edges.iter().filter(|e| e.source == id).collect()
    }
    #[must_use]
    pub fn incoming(&self, id: &str) -> Vec<&GraphEdge> {
        self.edges.iter().filter(|e| e.target == id).collect()
    }
    #[must_use]
    pub fn neighbors(&self, id: &str, kind: Option<&str>) -> Vec<&GraphNode> {
        self.outgoing(id)
            .into_iter()
            .filter(|e| kind.is_none_or(|k| e.kind == k))
            .filter_map(|e| self.node(&e.target))
            .collect()
    }
    #[must_use]
    pub fn nodes_defined_by_file(&self, path: &str) -> Vec<&GraphNode> {
        self.outgoing(&format!("file:{path}"))
            .into_iter()
            .filter(|e| e.kind == "DEFINES")
            .filter_map(|e| self.node(&e.target))
            .collect()
    }
    #[must_use]
    pub fn statistics(&self) -> (usize, usize) {
        (self.nodes.len(), self.edges.len())
    }
    #[must_use]
    pub fn find_symbols_exact(&self, name: &str) -> Vec<&GraphNode> {
        self.nodes
            .values()
            .filter(|node| {
                node.kind == NodeKind::Symbol && node.label.rsplit("::").next() == Some(name)
            })
            .collect()
    }
    #[must_use]
    pub fn find_symbols_prefix(&self, prefix: &str) -> Vec<&GraphNode> {
        self.nodes
            .values()
            .filter(|node| node.kind == NodeKind::Symbol && node.label.starts_with(prefix))
            .collect()
    }
    #[must_use]
    pub fn file_defining_node(&self, node_id: &str) -> Option<&str> {
        self.incoming(node_id)
            .into_iter()
            .find(|edge| edge.kind == "DEFINES")
            .and_then(|edge| edge.source.strip_prefix("file:"))
    }
    #[must_use]
    pub fn bounded_neighborhood(
        &self,
        start: &str,
        depth: usize,
        max_nodes: usize,
    ) -> Vec<&GraphNode> {
        let mut seen = BTreeSet::from([start.to_string()]);
        let mut frontier = vec![start.to_string()];
        let mut result = Vec::new();
        for _ in 0..=depth {
            let mut next = Vec::new();
            for current in frontier {
                for edge in self
                    .outgoing(&current)
                    .into_iter()
                    .chain(self.incoming(&current))
                {
                    if seen.insert(if edge.source == current {
                        edge.target.clone()
                    } else {
                        edge.source.clone()
                    }) {
                        let id = if edge.source == current {
                            &edge.target
                        } else {
                            &edge.source
                        };
                        if let Some(node) = self.node(id) {
                            result.push(node);
                            next.push(id.clone());
                            if result.len() >= max_nodes {
                                return result;
                            }
                        }
                    }
                }
            }
            frontier = next;
        }
        result
    }
}

fn semantic_provenance(mut provenance: Provenance) -> Provenance {
    provenance.source = "SyntaxResolver".into();
    provenance.analyzer = "repository-intelligence-rust-resolver".into();
    provenance.evidence = Evidence::Derived;
    provenance
}

fn rust_import_parts(target: &str) -> Option<(String, String)> {
    let target = target
        .trim()
        .strip_prefix("use ")?
        .trim_end_matches(';')
        .trim();
    if target.contains('{') || target.contains('*') {
        return None;
    }
    let mut parts = target.split("::").map(str::trim).collect::<Vec<_>>();
    let name = parts.pop()?.to_string();
    if parts.is_empty() {
        return None;
    }
    Some((name, parts.join("::")))
}

fn rust_module_path(path: &str, module: &str, paths: &BTreeSet<String>) -> Option<String> {
    let mut segments = module.split("::").collect::<Vec<_>>();
    let mut base = Path::new(path).parent().unwrap_or_else(|| Path::new(""));
    if segments.first().copied() == Some("crate") {
        segments.remove(0);
        base = Path::new("src");
    } else if segments.first().copied() == Some("self") {
        segments.remove(0);
    } else {
        while segments.first().copied() == Some("super") {
            segments.remove(0);
            base = base.parent().unwrap_or_else(|| Path::new(""));
        }
    }
    let relative = segments
        .iter()
        .fold(base.to_path_buf(), |mut path, segment| {
            path.push(segment);
            path
        });
    for candidate in [relative.with_extension("rs"), relative.join("mod.rs")] {
        let candidate = candidate.to_string_lossy().replace('\\', "/");
        if paths.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn project_provenance(bytes: &[u8], manifest_path: &str) -> Provenance {
    let content = ContentIdentity::from_bytes(bytes);
    let analysis = AnalysisIdentity::new(content, Language::Rust, &AnalysisConfig::default());
    Provenance {
        source: "ProjectMetadata".into(),
        analyzer: "repository-intelligence-manifest-reader".into(),
        analyzer_version: format!("project-metadata-1:{manifest_path}"),
        analysis_identity: analysis,
        evidence: Evidence::Exact,
    }
}

fn manifest_string_value(line: &str) -> Option<String> {
    let value = line.split_once('=')?.1.trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.split('"').next())
        .map(str::to_string)
}

fn cargo_dependency(line: &str, kind: &str, provenance: &Provenance) -> Option<ProjectDependency> {
    let (name, raw_value) = line.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || name.starts_with('#') {
        return None;
    }
    let raw_value = raw_value.trim();
    let mut requirement = raw_value.trim_matches('"').to_string();
    let mut local_manifest = None;
    let mut package_name = name.to_string();
    if raw_value.starts_with('{') {
        let inner = raw_value.trim_matches(|c| c == '{' || c == '}');
        for part in inner.split(',') {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            match key.trim() {
                "path" => local_manifest = Some(value.to_string()),
                "package" => package_name = value.to_string(),
                "version" => requirement = value.to_string(),
                _ => {}
            }
        }
        if requirement.starts_with('{') {
            requirement.clear();
        }
    }
    Some(ProjectDependency {
        name: package_name,
        requirement,
        kind: kind.into(),
        local_manifest,
        resolved_package: None,
        provenance: provenance.clone(),
    })
}

fn parse_cargo_manifest(
    root: &Path,
    path: &Path,
    bytes: &[u8],
    workspace_roots: &BTreeSet<String>,
) -> Option<ProjectFact> {
    let text = std::str::from_utf8(bytes).ok()?;
    let manifest_path = safe_relative(root, path).ok()?.display().to_string();
    let package_root = Path::new(&manifest_path)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .display()
        .to_string();
    let provenance = project_provenance(bytes, &manifest_path);
    let mut section = String::new();
    let mut package_name = None;
    let mut version = None;
    let mut dependencies = Vec::new();
    for line in text.lines() {
        let line = line.split_once('#').map_or(line, |(line, _)| line).trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).to_string();
            continue;
        }
        if section == "package" {
            if line.starts_with("name") {
                package_name = manifest_string_value(line);
            } else if line.starts_with("version") {
                version = manifest_string_value(line);
            }
        }
        if section == "dependencies"
            || section == "dev-dependencies"
            || section == "build-dependencies"
            || section.ends_with(".dependencies")
        {
            if let Some(dependency) = cargo_dependency(line, &section, &provenance) {
                dependencies.push(dependency);
            }
        }
    }
    let package_name = package_name?;
    let workspace_root = workspace_roots
        .iter()
        .find(|workspace| {
            workspace.is_empty()
                || package_root == **workspace
                || package_root.starts_with(&format!("{workspace}/"))
        })
        .cloned();
    Some(ProjectFact {
        manifest_path,
        ecosystem: "Cargo".into(),
        package_name,
        package_root,
        workspace_root,
        version,
        dependencies,
        provenance,
    })
}

fn parse_package_json(root: &Path, path: &Path, bytes: &[u8]) -> Option<ProjectFact> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let package_name = value.get("name")?.as_str()?.to_string();
    let manifest_path = safe_relative(root, path).ok()?.display().to_string();
    let package_root = Path::new(&manifest_path)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .display()
        .to_string();
    let provenance = project_provenance(bytes, &manifest_path);
    let mut dependencies = Vec::new();
    for (field, kind) in [
        ("dependencies", "dependencies"),
        ("devDependencies", "dev-dependencies"),
        ("peerDependencies", "peer-dependencies"),
        ("optionalDependencies", "optional-dependencies"),
    ] {
        if let Some(values) = value.get(field).and_then(serde_json::Value::as_object) {
            for (name, requirement) in values {
                dependencies.push(ProjectDependency {
                    name: name.clone(),
                    requirement: requirement.as_str().unwrap_or("*").into(),
                    kind: kind.into(),
                    local_manifest: None,
                    resolved_package: None,
                    provenance: provenance.clone(),
                });
            }
        }
    }
    Some(ProjectFact {
        manifest_path,
        ecosystem: "Node".into(),
        package_name,
        package_root,
        workspace_root: None,
        version: value
            .get("version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        dependencies,
        provenance,
    })
}

fn project_facts(root: &Path) -> std::io::Result<Vec<ProjectFact>> {
    let mut manifests = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file()
            || entry
                .path()
                .components()
                .any(|c| matches!(c, Component::Normal(v) if v == ".git"))
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name == "Cargo.toml" || name == "package.json" {
            manifests.push(entry.path().to_path_buf());
        }
    }
    manifests.sort();
    let mut workspace_roots = BTreeSet::new();
    for path in &manifests {
        if path.file_name().is_some_and(|name| name == "Cargo.toml") {
            let bytes = fs::read(path)?;
            if std::str::from_utf8(&bytes)
                .is_ok_and(|text| text.lines().any(|line| line.trim() == "[workspace]"))
            {
                if let Ok(relative) = safe_relative(root, path) {
                    workspace_roots.insert(
                        relative
                            .parent()
                            .unwrap_or_else(|| Path::new(""))
                            .display()
                            .to_string(),
                    );
                }
            }
        }
    }
    let mut facts = Vec::new();
    for path in manifests {
        let bytes = fs::read(&path)?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES || bytes.contains(&0) {
            continue;
        }
        let fact = if path.file_name().is_some_and(|name| name == "Cargo.toml") {
            parse_cargo_manifest(root, &path, &bytes, &workspace_roots)
        } else {
            parse_package_json(root, &path, &bytes)
        };
        if let Some(fact) = fact {
            facts.push(fact);
        }
    }
    let package_ids: BTreeMap<String, String> = facts
        .iter()
        .map(|fact| {
            (
                fact.manifest_path.clone(),
                format!("package:{}:{}", fact.manifest_path, fact.package_name),
            )
        })
        .collect();
    for fact in &mut facts {
        for dependency in &mut fact.dependencies {
            let Some(local) = dependency.local_manifest.as_ref() else {
                continue;
            };
            let manifest = normalize_project_path(
                &Path::new(&fact.package_root).join(local).join("Cargo.toml"),
            );
            dependency.local_manifest = Some(manifest.clone());
            dependency.resolved_package = package_ids.get(&manifest).cloned();
        }
    }
    facts.sort_by(|a, b| a.manifest_path.cmp(&b.manifest_path));
    Ok(facts)
}

fn normalize_project_path(path: &Path) -> String {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            Component::ParentDir => {
                let _ = parts.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    parts.join("/")
}

#[derive(Clone, Debug)]
pub struct RepositoryIndex {
    pub root: PathBuf,
    pub canonical: RepositoryGraph,
    pub candidate: Option<RepositoryGraph>,
    pub config: AnalysisConfig,
    pub private: bool,
}

impl RepositoryIndex {
    pub fn build(
        root: &Path,
        cache_dir: Option<&Path>,
        config: AnalysisConfig,
        private: bool,
    ) -> std::io::Result<BuildOutput> {
        let started = Instant::now();
        let mut graph = RepositoryGraph::default();
        let mut stats = BuildStats::default();
        let mut skipped = Vec::new();
        let repo_id = "repository".to_string();
        graph.nodes.insert(
            repo_id.clone(),
            GraphNode {
                id: repo_id,
                kind: NodeKind::Repository,
                label: "repository".into(),
                provenance: None,
            },
        );
        let mut paths = Vec::new();
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = match safe_relative(root, entry.path()) {
                Ok(p) => p,
                Err(_) => {
                    skipped.push(SkippedFile {
                        relative_path: entry.path().display().to_string(),
                        reason: SkipReason::InvalidPath,
                    });
                    continue;
                }
            };
            if relative
                .components()
                .any(|c| matches!(c,Component::Normal(v) if v==".git"))
            {
                continue;
            }
            if language_for_path(&relative).is_some() {
                paths.push((relative, entry.path().to_path_buf()));
            }
        }
        paths.sort();
        stats.supported_files = paths.len();
        for (relative, path) in paths {
            let bytes = fs::read(&path)?;
            if bytes.len() as u64 > config.max_source_bytes {
                stats.files_skipped += 1;
                skipped.push(SkippedFile {
                    relative_path: relative.display().to_string(),
                    reason: SkipReason::Oversize,
                });
                continue;
            }
            if bytes.contains(&0) {
                stats.files_skipped += 1;
                skipped.push(SkippedFile {
                    relative_path: relative.display().to_string(),
                    reason: SkipReason::Binary,
                });
                continue;
            }
            let language = language_for_path(&relative).expect("filtered above");
            let identity =
                AnalysisIdentity::new(ContentIdentity::from_bytes(&bytes), language, &config);
            let cache_path = cache_dir
                .filter(|_| !private)
                .map(|dir| dir.join(format!("{identity}.json")));
            let pack = cache_path
                .as_ref()
                .and_then(|p| fs::read(p).ok())
                .and_then(|b| serde_json::from_slice::<FileFactPack>(&b).ok())
                .filter(|p| p.analysis_identity == identity)
                .map(|mut p| {
                    p.relative_path = relative.display().to_string();
                    stats.cache_hits += 1;
                    p
                });
            let pack = match pack {
                Some(p) => p,
                None => {
                    stats.cache_misses += 1;
                    stats.parsed_files += 1;
                    let p =
                        extract_file(&relative.display().to_string(), language, &bytes, &config);
                    if let Some(path) = cache_path {
                        if let Some(parent) = path.parent() {
                            let _ = fs::create_dir_all(parent);
                        }
                        if let Ok(data) = serde_json::to_vec(&p) {
                            let temp = path.with_extension("tmp");
                            if fs::write(&temp, data).is_ok() {
                                let _ = fs::rename(temp, path);
                            }
                        }
                    }
                    p
                }
            };
            graph.add_pack(pack);
        }
        for fact in project_facts(root)? {
            graph.add_project_fact(fact);
        }
        graph.enrich_semantics();
        graph.finish();
        let (nodes, edges) = graph.statistics();
        stats.nodes = nodes;
        stats.edges = edges;
        stats.build_ms = started.elapsed().as_millis();
        Ok(BuildOutput {
            graph: graph.clone(),
            stats,
            skipped,
        })
    }
    pub fn refresh_candidate(&mut self, candidate_root: &Path) -> std::io::Result<BuildStats> {
        let mut merged = self.canonical.clone();
        let started = Instant::now();
        let (paths, skipped) = discover_supported_paths(candidate_root);
        let mut stats = BuildStats {
            supported_files: paths.len(),
            files_skipped: skipped.len(),
            ..BuildStats::default()
        };
        let mut candidate_paths = BTreeSet::new();
        for (relative, path) in paths {
            let bytes = fs::read(&path)?;
            let relative = relative.display().to_string();
            let Some(language) = language_for_path(Path::new(&relative)) else {
                continue;
            };
            if bytes.len() as u64 > self.config.max_source_bytes || bytes.contains(&0) {
                continue;
            }
            candidate_paths.insert(relative.clone());
            let identity =
                AnalysisIdentity::new(ContentIdentity::from_bytes(&bytes), language, &self.config);
            if self
                .canonical
                .facts
                .get(&relative)
                .is_some_and(|pack| pack.analysis_identity == identity)
            {
                stats.cache_hits += 1;
                continue;
            }
            stats.cache_misses += 1;
            stats.parsed_files += 1;
            merged.remove_file(&relative);
            merged.add_pack(extract_file(&relative, language, &bytes, &self.config));
        }
        for path in self.canonical.facts.keys().cloned().collect::<Vec<_>>() {
            if !candidate_paths.contains(&path) {
                merged.remove_file(&path);
            }
        }
        merged.clear_project_metadata();
        for fact in project_facts(candidate_root)? {
            merged.add_project_fact(fact);
        }
        merged.enrich_semantics();
        merged.finish();
        let (nodes, edges) = merged.statistics();
        stats.nodes = nodes;
        stats.edges = edges;
        stats.build_ms = started.elapsed().as_millis();
        self.candidate = Some(merged);
        Ok(stats)
    }
    pub fn discard_candidate(&mut self) {
        self.candidate = None;
    }
    #[must_use]
    pub fn active_graph(&self) -> Option<&RepositoryGraph> {
        self.candidate.as_ref().or(Some(&self.canonical))
    }
}

fn safe_relative(root: &Path, path: &Path) -> Result<PathBuf, SkipReason> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| SkipReason::InvalidPath)?;
    if rel.is_absolute()
        || rel.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(SkipReason::InvalidPath);
    }
    Ok(rel.to_path_buf())
}

fn discover_supported_paths(root: &Path) -> (Vec<(PathBuf, PathBuf)>, Vec<SkippedFile>) {
    let mut paths = Vec::new();
    let mut skipped = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = match safe_relative(root, entry.path()) {
            Ok(path) => path,
            Err(_) => {
                skipped.push(SkippedFile {
                    relative_path: entry.path().display().to_string(),
                    reason: SkipReason::InvalidPath,
                });
                continue;
            }
        };
        if relative
            .components()
            .any(|component| matches!(component, Component::Normal(value) if value == ".git"))
        {
            continue;
        }
        if language_for_path(&relative).is_some() {
            paths.push((relative, entry.path().to_path_buf()));
        }
    }
    paths.sort();
    (paths, skipped)
}

fn pack_provenance(pack: &FileFactPack) -> Provenance {
    Provenance {
        source: "TreeSitter".into(),
        analyzer: "repository-intelligence".into(),
        analyzer_version: EXTRACTOR_VERSION.into(),
        analysis_identity: pack.analysis_identity,
        evidence: Evidence::Exact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_identity_is_path_independent() {
        assert_eq!(
            ContentIdentity::from_bytes(b"x"),
            ContentIdentity::from_bytes(b"x")
        );
        assert_ne!(
            ContentIdentity::from_bytes(b"x"),
            ContentIdentity::from_bytes(b"y")
        );
    }
    #[test]
    fn language_facts_cover_target_registry() {
        let cases = [
            (
                "a.rs",
                Language::Rust,
                "struct A {} fn f() {} use crate::x;",
            ),
            (
                "a.py",
                Language::Python,
                "class A:\n def f(self): pass\nimport os",
            ),
            (
                "a.java",
                Language::Java,
                "package p; class A { void f() {} } import x.Y;",
            ),
            (
                "a.ts",
                Language::TypeScript,
                "interface A {} function f() {} import x from 'x';",
            ),
            (
                "a.js",
                Language::JavaScript,
                "class A {} function f() {} export { A };",
            ),
            (
                "a.tsx",
                Language::Tsx,
                "interface Props {} const A = () => <div />;",
            ),
            (
                "a.go",
                Language::Go,
                "package p\ntype A struct{}\nfunc f() {}",
            ),
            ("a.c", Language::C, "#include <x>\nstruct A {}; int f() {}"),
            ("a.cpp", Language::Cpp, "namespace n { class A {}; }"),
            ("a.cs", Language::CSharp, "namespace N { class A {} }"),
            ("a.html", Language::Html, "<div id='x' class='y'></div>"),
            ("a.css", Language::Css, "@import 'x'; .a { --x: 1; }"),
        ];
        for (path, lang, src) in cases {
            let pack = extract_file(path, lang, src.as_bytes(), &AnalysisConfig::default());
            assert_eq!(pack.language, lang);
            assert!(!pack.analysis_identity.0.iter().all(|b| *b == 0));
        }
    }
    #[test]
    fn malformed_source_keeps_partial_facts() {
        let p = extract_file(
            "a.rs",
            Language::Rust,
            b"fn good( {",
            &AnalysisConfig::default(),
        );
        assert!(p.parse_diagnostics.contains_error_nodes);
    }
    #[test]
    fn analysis_identity_changes_with_config() {
        let a = ContentIdentity::from_bytes(b"x");
        assert_ne!(
            AnalysisIdentity::new(
                a,
                Language::Rust,
                &AnalysisConfig {
                    max_source_bytes: 1
                }
            ),
            AnalysisIdentity::new(
                a,
                Language::Rust,
                &AnalysisConfig {
                    max_source_bytes: 2
                }
            )
        );
    }
    #[test]
    fn graph_overlay_shadows_and_discard_restores() {
        let dir = std::env::temp_dir().join(format!("ri-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("a.rs"), "fn old() {}").unwrap();
        let built = RepositoryIndex::build(&dir, None, AnalysisConfig::default(), true).unwrap();
        let mut idx = RepositoryIndex {
            root: dir.clone(),
            canonical: built.graph,
            candidate: None,
            config: AnalysisConfig::default(),
            private: true,
        };
        let cand = dir.join("candidate");
        fs::create_dir_all(&cand).unwrap();
        fs::write(cand.join("a.rs"), "fn new() {}").unwrap();
        idx.refresh_candidate(&cand).unwrap();
        assert!(idx
            .active_graph()
            .unwrap()
            .file_facts("a.rs")
            .unwrap()
            .symbols
            .iter()
            .any(|s| s.name == "new"));
        idx.discard_candidate();
        assert!(idx
            .active_graph()
            .unwrap()
            .file_facts("a.rs")
            .unwrap()
            .symbols
            .iter()
            .any(|s| s.name == "old"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cache_reuses_matching_packs_and_rebuilds_corrupt_entries() {
        let dir = std::env::temp_dir().join(format!("ri-cache-{}", std::process::id()));
        let cache = dir.join("cache");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.rs"), "fn a() {}").unwrap();
        let cold =
            RepositoryIndex::build(&dir, Some(&cache), AnalysisConfig::default(), false).unwrap();
        assert_eq!(cold.stats.cache_misses, 1);
        let warm =
            RepositoryIndex::build(&dir, Some(&cache), AnalysisConfig::default(), false).unwrap();
        assert_eq!(warm.stats.cache_hits, 1);
        let cache_file = fs::read_dir(&cache)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(cache_file, b"corrupt").unwrap();
        let rebuilt =
            RepositoryIndex::build(&dir, Some(&cache), AnalysisConfig::default(), false).unwrap();
        assert_eq!(rebuilt.stats.cache_misses, 1);
        let private_cache = dir.join("private-cache");
        RepositoryIndex::build(&dir, Some(&private_cache), AnalysisConfig::default(), true)
            .unwrap();
        assert!(!private_cache.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn candidate_refresh_reuses_unchanged_files_and_handles_add_delete() {
        let dir = std::env::temp_dir().join(format!("ri-refresh-{}", std::process::id()));
        let candidate = dir.join("candidate");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&candidate).unwrap();
        fs::write(dir.join("a.rs"), "fn old() {}").unwrap();
        fs::write(dir.join("b.py"), "def b(): pass\n").unwrap();
        fs::write(candidate.join("a.rs"), "fn new() {}").unwrap();
        fs::write(candidate.join("b.py"), "def b(): pass\n").unwrap();
        fs::write(candidate.join("c.go"), "package p\nfunc c() {}\n").unwrap();
        let built = RepositoryIndex::build(&dir, None, AnalysisConfig::default(), true).unwrap();
        let mut index = RepositoryIndex {
            root: dir.clone(),
            canonical: built.graph,
            candidate: None,
            config: AnalysisConfig::default(),
            private: true,
        };
        let refreshed = index.refresh_candidate(&candidate).unwrap();
        assert_eq!(refreshed.cache_hits, 1);
        assert_eq!(refreshed.parsed_files, 2);
        assert!(index
            .active_graph()
            .unwrap()
            .file_facts("a.rs")
            .unwrap()
            .symbols
            .iter()
            .any(|s| s.name == "new"));
        assert!(index.active_graph().unwrap().file_facts("c.go").is_some());
        fs::remove_file(candidate.join("b.py")).unwrap();
        index.refresh_candidate(&candidate).unwrap();
        assert!(index.active_graph().unwrap().file_facts("b.py").is_none());
        index.discard_candidate();
        assert!(index.active_graph().unwrap().file_facts("b.py").is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn extraction_and_graph_order_are_deterministic() {
        let config = AnalysisConfig::default();
        let left = extract_file("src/a.rs", Language::Rust, b"fn b() {} fn a() {}", &config);
        let right = extract_file("src/a.rs", Language::Rust, b"fn b() {} fn a() {}", &config);
        assert_eq!(left, right);
        let mut graph = RepositoryGraph::default();
        graph.add_pack(left);
        graph.finish();
        assert!(graph.node("file:src/a.rs").is_some());
        assert!(graph
            .outgoing("file:src/a.rs")
            .iter()
            .all(|edge| graph.node(&edge.target).is_some()));
    }

    #[test]
    fn unsafe_relative_paths_are_rejected() {
        let root = Path::new("/tmp/repository");
        assert!(safe_relative(root, Path::new("/tmp/repository/ok.rs")).is_ok());
        assert!(safe_relative(root, Path::new("/tmp/repository/../outside.rs")).is_err());
        assert!(safe_relative(root, Path::new("/other/ok.rs")).is_err());
    }

    #[test]
    fn rust_semantics_resolve_explicit_imports_not_unrelated_names() {
        let dir = std::env::temp_dir().join(format!("ri-semantic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/main.rs"),
            "use crate::util::target; fn main() { target(); }",
        )
        .unwrap();
        fs::write(dir.join("src/util.rs"), "pub fn target() {}").unwrap();
        fs::write(dir.join("src/other.rs"), "pub fn target() {}").unwrap();
        let built = RepositoryIndex::build(&dir, None, AnalysisConfig::default(), true).unwrap();
        let main = built.graph.file_facts("src/main.rs").unwrap();
        let util_id = "symbol:src/util.rs:target";
        assert!(main
            .imports
            .iter()
            .any(|import| import.resolution == util_id));
        let references = built
            .graph
            .outgoing("file:src/main.rs")
            .into_iter()
            .filter(|edge| edge.kind == "REFERENCES")
            .collect::<Vec<_>>();
        assert!(!references.is_empty());
        assert!(references.iter().all(|edge| edge.target == util_id));
        assert!(references.iter().all(|edge| {
            edge.provenance.source == "SyntaxResolver"
                && edge.provenance.evidence == Evidence::Derived
        }));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn project_metadata_resolves_local_cargo_dependency_and_tracks_external() {
        let dir = std::env::temp_dir().join(format!("ri-project-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("crates/app/src")).unwrap();
        fs::create_dir_all(dir.join("crates/core/src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\", \"crates/core\"]\n",
        )
        .unwrap();
        fs::write(
            dir.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n\n[dependencies]\ncore = { path = \"../core\" }\nserde = \"1\"\n",
        )
        .unwrap();
        fs::write(
            dir.join("crates/core/Cargo.toml"),
            "[package]\nname = \"core\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(dir.join("crates/app/src/main.rs"), "fn main() {}").unwrap();
        fs::write(dir.join("crates/core/src/lib.rs"), "pub fn core() {}").unwrap();
        let output = RepositoryIndex::build(&dir, None, AnalysisConfig::default(), true).unwrap();
        let app = output
            .graph
            .project_facts
            .get("crates/app/Cargo.toml")
            .unwrap();
        let dependency = app
            .dependencies
            .iter()
            .find(|dep| dep.name == "core")
            .unwrap();
        assert_eq!(
            dependency.resolved_package.as_deref(),
            Some("package:crates/core/Cargo.toml:core")
        );
        assert!(app
            .dependencies
            .iter()
            .any(|dep| dep.name == "serde" && dep.resolved_package.is_none()));
        assert!(app.workspace_root.is_some());
        assert!(output
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == "DEPENDS_ON"
                && edge.target == "package:crates/core/Cargo.toml:core"));
        let _ = fs::remove_dir_all(dir);
    }
}
