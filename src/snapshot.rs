use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use indexmap::IndexMap;
use mago_allocator::LocalArena;
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::populator::populate_codebase;
use mago_codex::reference::SymbolReferences;
use mago_codex::scanner::scan_program;
use mago_database::file::{File, FileType};
use mago_database::{Database, DatabaseReader};
use mago_names::{ResolvedNames, resolver::NameResolver};
use mago_php_version::PHPVersion;
use mago_prelude::Prelude;
use mago_span::{HasSpan, Span};
use mago_syntax::comments::docblock::get_docblock_before_position;
use mago_syntax::cst::{
    Class, ClassLikeMember, Enum, EnumCaseItem, Hint, Identifier, Interface, MethodBody, Modifier, Program,
    Property as AstProperty, PropertyItem, Trait, TraitUseAdaptation, TraitUseMethodReference, TraitUseSpecification,
};
use mago_syntax::parser::parse_file_with_settings;
use mago_syntax::settings::ParserSettings;
use mago_syntax::walker::Walker;
use mago_word::{ascii_lowercase_word, word};

use crate::change::SourceLocation;
use crate::config::Configuration;
use crate::error::CheckError;
use crate::value::{PhpValue, value_from_expression};

const PRELUDE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/prelude.bin"));
static BUILTIN_CLASS_LIKES: OnceLock<Result<IndexMap<String, ClassLike>, String>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRole {
    Project,
    Dependency,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub path: PathBuf,
    pub role: SourceRole,
}

impl SourceFile {
    #[must_use]
    pub fn new(root: &Path, path: impl Into<PathBuf>, role: SourceRole) -> Self {
        let path = path.into();
        let path = if path.is_absolute() { path } else { root.join(path) };
        Self { path, role }
    }

    #[must_use]
    pub fn project(root: &Path, path: impl Into<PathBuf>) -> Self {
        Self::new(root, path, SourceRole::Project)
    }

    #[must_use]
    pub fn dependency(root: &Path, path: impl Into<PathBuf>) -> Self {
        Self::new(root, path, SourceRole::Dependency)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassLikeKind {
    Class,
    Interface,
    Trait,
    Enum,
}

impl fmt::Display for ClassLikeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Trait => "trait",
            Self::Enum => "enum",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Visibility {
    Private,
    Protected,
    Public,
}

impl fmt::Display for Visibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Private => "private",
            Self::Protected => "protected",
            Self::Public => "public",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeType {
    Named(String),
    SelfType,
    ParentType,
    StaticType,
    Nullable(Box<NativeType>),
    Union(Vec<NativeType>),
    Intersection(Vec<NativeType>),
    Parenthesized(Box<NativeType>),
}

impl fmt::Display for NativeType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(name) => formatter.write_str(name),
            Self::SelfType => formatter.write_str("self"),
            Self::ParentType => formatter.write_str("parent"),
            Self::StaticType => formatter.write_str("static"),
            Self::Nullable(inner) => write!(formatter, "?{inner}"),
            Self::Union(types) => write_joined(formatter, types, "|"),
            Self::Intersection(types) => write_joined(formatter, types, "&"),
            Self::Parenthesized(inner) => write!(formatter, "({inner})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitUse {
    pub traits: Vec<String>,
    pub aliases: Vec<TraitAlias>,
    pub precedences: Vec<TraitPrecedence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitAlias {
    pub trait_name: Option<String>,
    pub method: String,
    pub alias: Option<String>,
    pub visibility: Option<Visibility>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitPrecedence {
    pub trait_name: String,
    pub method: String,
    pub instead_of: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    pub position: usize,
    pub location: SourceLocation,
    pub native_type: Option<NativeType>,
    pub by_reference: bool,
    pub variadic: bool,
    pub has_default: bool,
    pub default_value: Option<PhpValue>,
    pub promoted_visibility: Option<Visibility>,
    pub promoted_readonly: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method {
    pub name: String,
    pub declaring_class: String,
    pub location: SourceLocation,
    pub docblock: Option<String>,
    pub is_internal: bool,
    pub no_named_arguments: bool,
    pub visibility: Visibility,
    pub is_static: bool,
    pub is_final: bool,
    pub is_abstract: bool,
    pub returns_by_reference: bool,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<NativeType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    pub name: String,
    pub declaring_class: String,
    pub location: SourceLocation,
    pub docblock: Option<String>,
    pub is_internal: bool,
    pub visibility: Visibility,
    pub is_static: bool,
    pub is_readonly: bool,
    pub native_type: Option<NativeType>,
    pub default_value: PhpValue,
    pub promoted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassConstant {
    pub name: String,
    pub declaring_class: String,
    pub location: SourceLocation,
    pub visibility: Visibility,
    pub is_final: bool,
    pub native_type: Option<NativeType>,
    pub value: PhpValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumCase {
    pub name: String,
    pub declaring_class: String,
    pub location: SourceLocation,
    pub docblock: Option<String>,
    pub is_internal: bool,
    pub value: Option<PhpValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassLike {
    pub name: String,
    pub kind: ClassLikeKind,
    pub role: SourceRole,
    pub location: SourceLocation,
    pub docblock: Option<String>,
    pub is_internal: bool,
    pub is_final: bool,
    pub is_abstract: bool,
    pub is_readonly: bool,
    pub extends: Vec<String>,
    pub implements: Vec<String>,
    pub trait_uses: Vec<TraitUse>,
    pub enum_backing_type: Option<NativeType>,
    pub methods: IndexMap<String, Method>,
    pub properties: IndexMap<String, Property>,
    pub constants: IndexMap<String, ClassConstant>,
    pub enum_cases: IndexMap<String, EnumCase>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    _root: PathBuf,
    pub class_likes: IndexMap<String, ClassLike>,
    builtin_class_likes: &'static IndexMap<String, ClassLike>,
    pub codebase: CodebaseMetadata,
    _symbol_references: SymbolReferences,
}

#[derive(Debug, Clone)]
pub struct ResolvedMethod<'a> {
    pub method: &'a Method,
    pub name: String,
    pub visibility: Visibility,
    pub declaring_class: &'a str,
    pub implementing_class: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedProperty<'a> {
    pub property: &'a Property,
    pub declaring_class: &'a str,
    pub implementing_class: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedConstant<'a> {
    pub constant: &'a ClassConstant,
    pub declaring_class: &'a str,
    pub implementing_class: &'a str,
}

fn builtin_class_likes(prelude: &Prelude) -> Result<&'static IndexMap<String, ClassLike>, CheckError> {
    match BUILTIN_CLASS_LIKES.get_or_init(|| capture_builtin_class_likes(&prelude.database, &prelude.metadata)) {
        Ok(class_likes) => Ok(class_likes),
        Err(error) => Err(CheckError::Extraction(error.clone())),
    }
}

fn capture_builtin_class_likes(
    database: &Database<'_>,
    codebase: &CodebaseMetadata,
) -> Result<IndexMap<String, ClassLike>, String> {
    let mut files = database.files().collect::<Vec<_>>();
    files.sort_unstable_by_key(|file| file.id);

    let mut class_likes = IndexMap::new();
    let mut arena = LocalArena::new();
    for file in files {
        arena.reset();
        let program = parse_file_with_settings(&arena, &file, ParserSettings::default());
        if program.has_errors() {
            return Err(format!(
                "embedded PHP prelude file {} has parse errors: {:?}",
                bytes_string(file.name.as_ref()),
                program.errors
            ));
        }

        let resolved_names = NameResolver::new(&arena).resolve(program);
        let logical_path = PathBuf::new();
        for mut class_like in capture_program(&file, &logical_path, SourceRole::Dependency, program, &resolved_names) {
            if !retain_php85_builtin_members(&mut class_like, &file, codebase) {
                continue;
            }
            apply_builtin_reflection_defaults(&mut class_like);
            class_likes.entry(symbol_key(&class_like.name)).or_insert(class_like);
        }
    }

    Ok(class_likes)
}

fn retain_php85_builtin_members(class_like: &mut ClassLike, file: &File, codebase: &CodebaseMetadata) -> bool {
    let Some(metadata) = codebase.get_class_like(class_like.name.as_bytes()) else {
        return false;
    };
    if metadata.span.file_id != file.id || !metadata.is_available_in_version(PHPVersion::PHP85) {
        return false;
    }

    class_like.methods.retain(|_, method| {
        codebase
            .get_method(class_like.name.as_bytes(), method.name.as_bytes())
            .is_some_and(|metadata| {
                metadata.span.file_id == file.id && metadata.is_available_in_version(PHPVersion::PHP85)
            })
    });
    class_like.properties.retain(|_, property| {
        let name = format!("${}", property.name);
        codebase
            .get_property(class_like.name.as_bytes(), name.as_bytes())
            .is_some_and(|metadata| {
                metadata.span.is_none_or(|span| span.file_id == file.id)
                    && metadata.is_available_in_version(PHPVersion::PHP85)
            })
    });
    class_like.constants.retain(|_, constant| {
        metadata
            .constants
            .get(&word(constant.name.as_bytes()))
            .is_some_and(|metadata| {
                metadata.span.file_id == file.id && metadata.is_available_in_version(PHPVersion::PHP85)
            })
    });
    class_like.enum_cases.retain(|_, case| {
        metadata
            .enum_cases
            .get(&word(case.name.as_bytes()))
            .is_some_and(|metadata| {
                metadata.span.file_id == file.id && metadata.is_available_in_version(PHPVersion::PHP85)
            })
    });
    true
}

fn apply_builtin_reflection_defaults(class_like: &mut ClassLike) {
    if !matches!(symbol_key(&class_like.name).as_str(), "exception" | "error") {
        return;
    }

    // These values are assigned by the engine and are absent from Mago's declaration stubs.
    for property in class_like.properties.values_mut() {
        property.default_value = match property.name.as_str() {
            "message" | "file" => PhpValue::String(Vec::new()),
            "code" => PhpValue::Integer(0),
            "line" if class_like.name.eq_ignore_ascii_case("Exception") => PhpValue::Integer(0),
            _ => continue,
        };
    }
}

impl Snapshot {
    #[cfg(test)]
    pub fn build(root: impl Into<PathBuf>, sources: impl IntoIterator<Item = SourceFile>) -> Result<Self, CheckError> {
        Self::build_with_project_exclusions(root, sources, |_| false)
    }

    pub(crate) fn build_with_configuration(
        root: impl Into<PathBuf>,
        sources: impl IntoIterator<Item = SourceFile>,
        configuration: &Configuration,
    ) -> Result<Self, CheckError> {
        Self::build_with_project_exclusions(root, sources, |path| configuration.excludes_project_path(path))
    }

    fn build_with_project_exclusions(
        root: impl Into<PathBuf>,
        sources: impl IntoIterator<Item = SourceFile>,
        excludes_project_path: impl Fn(&Path) -> bool,
    ) -> Result<Self, CheckError> {
        let root = root.into();
        let sources = expand_sources(&root, sources, &excludes_project_path)?;
        let prelude = Prelude::decode(PRELUDE)
            .map_err(|error| CheckError::Extraction(format!("unable to decode the embedded PHP prelude: {error}")))?;
        let builtin_class_likes = builtin_class_likes(&prelude)?;
        let mut codebase = prelude.metadata;
        let mut symbol_references = prelude.symbol_references;
        let mut class_likes: IndexMap<String, ClassLike> = IndexMap::new();
        let mut partials = Vec::with_capacity(sources.len());
        let mut arena = LocalArena::new();

        for source in sources {
            arena.reset();
            let contents = fs::read(&source.path)?;
            let logical_path = logical_path(&root, &source.path);
            let file_type = match source.role {
                SourceRole::Project => FileType::Host,
                SourceRole::Dependency => FileType::Vendored,
            };
            let file = File::new(
                Cow::Owned(path_bytes(&logical_path)),
                file_type,
                Some(source.path.clone()),
                Cow::Owned(contents),
            );
            let program = parse_file_with_settings(&arena, &file, ParserSettings::default());
            if program.has_errors() {
                return Err(CheckError::Parse(format!(
                    "{}: {:?}",
                    source.path.display(),
                    program.errors
                )));
            }

            let resolved_names = NameResolver::new(&arena).resolve(program);
            let captured = capture_program(&file, &logical_path, source.role, program, &resolved_names);
            for class_like in captured {
                let key = symbol_key(&class_like.name);
                match class_likes.get(&key) {
                    Some(existing)
                        if existing.role == SourceRole::Project || class_like.role == SourceRole::Dependency => {}
                    _ => {
                        class_likes.insert(key, class_like);
                    }
                }
            }
            partials.push(scan_program(&arena, &file, program, &resolved_names, PHPVersion::PHP85));
        }

        for partial in partials {
            codebase.extend(partial);
        }
        codebase.apply_patches_pass();
        populate_codebase(
            &mut codebase,
            &mut symbol_references,
            Default::default(),
            Default::default(),
        );

        Ok(Self {
            _root: root,
            class_likes,
            builtin_class_likes,
            codebase,
            _symbol_references: symbol_references,
        })
    }

    #[must_use]
    pub fn class_like(&self, name: &str) -> Option<&ClassLike> {
        let key = symbol_key(name);
        self.class_likes
            .get(&key)
            .or_else(|| self.builtin_class_likes.get(&key))
    }

    #[must_use]
    pub fn is_interface(&self, name: &str) -> bool {
        self.class_like(name)
            .is_some_and(|class_like| class_like.kind == ClassLikeKind::Interface)
            || self
                .codebase
                .class_likes
                .get(&ascii_lowercase_word(name.as_bytes()))
                .is_some_and(|class_like| class_like.kind.is_interface())
    }

    /// Return the transitive class/interface ancestry in declaration order.
    #[must_use]
    pub fn ancestor_names(&self, name: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        self.collect_ancestors(name, &mut visited, &mut result);
        if let Some(metadata) = self.codebase.class_likes.get(&ascii_lowercase_word(name.as_bytes())) {
            for ancestor in metadata
                .all_parent_classes
                .iter()
                .chain(&metadata.all_parent_interfaces)
            {
                let ancestor = bytes_string(ancestor.as_bytes());
                if visited.insert(symbol_key(&ancestor)) {
                    result.push(ancestor);
                }
            }
        }
        result
    }

    #[must_use]
    pub fn parent_class_names(&self, name: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut current = self.class_like(name);
        let mut visited = HashSet::new();
        while let Some(class_like) = current {
            if class_like.kind != ClassLikeKind::Class {
                break;
            }
            let Some(parent) = class_like.extends.first() else {
                break;
            };
            if !visited.insert(symbol_key(parent)) {
                break;
            }
            result.push(parent.clone());
            current = self.class_like(parent);
        }
        if let Some(metadata) = self.codebase.class_likes.get(&ascii_lowercase_word(name.as_bytes())) {
            for parent in &metadata.all_parent_classes {
                let parent = bytes_string(parent.as_bytes());
                if visited.insert(symbol_key(&parent)) {
                    result.push(parent);
                }
            }
        }
        result
    }

    #[must_use]
    pub fn implements_interface(&self, name: &str, interface: &str) -> bool {
        self.codebase.class_implements(name.as_bytes(), interface.as_bytes()) || {
            let interface_key = symbol_key(interface);
            self.ancestor_names(name)
                .iter()
                .any(|ancestor| symbol_key(ancestor) == interface_key)
        }
    }

    #[must_use]
    pub fn extends_class(&self, name: &str, parent: &str) -> bool {
        self.codebase.class_extends(name.as_bytes(), parent.as_bytes())
            || self
                .parent_class_names(name)
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(parent))
    }

    #[must_use]
    pub fn is_instance_of(&self, name: &str, parent: &str) -> bool {
        name.eq_ignore_ascii_case(parent)
            || self.codebase.is_instance_of(name.as_bytes(), parent.as_bytes())
            || self.extends_class(name, parent)
            || self.implements_interface(name, parent)
    }

    /// Materialize the methods visible through a class-like, including imported
    /// trait methods and inherited methods. Keys are case-insensitive PHP names.
    #[must_use]
    pub fn methods_of(&self, name: &str) -> IndexMap<String, ResolvedMethod<'_>> {
        let mut result = IndexMap::new();
        self.collect_methods(name, None, &mut HashSet::new(), &mut result);
        result
    }

    /// Materialize declared, trait-imported, and inherited properties.
    #[must_use]
    pub fn properties_of(&self, name: &str) -> IndexMap<String, ResolvedProperty<'_>> {
        let mut result = IndexMap::new();
        self.collect_properties(name, None, &mut HashSet::new(), &mut result);
        result
    }

    /// Materialize declared, trait-imported, and inherited class constants.
    #[must_use]
    pub fn constants_of(&self, name: &str) -> IndexMap<String, ResolvedConstant<'_>> {
        let mut result = IndexMap::new();
        self.collect_constants(name, None, &mut HashSet::new(), &mut result);
        result
    }

    fn collect_ancestors(&self, name: &str, visited: &mut HashSet<String>, result: &mut Vec<String>) {
        let Some(class_like) = self.class_like(name) else {
            return;
        };
        for ancestor in class_like.extends.iter().chain(&class_like.implements) {
            let key = symbol_key(ancestor);
            if visited.insert(key) {
                result.push(ancestor.clone());
                self.collect_ancestors(ancestor, visited, result);
            }
        }
    }

    fn collect_methods<'a>(
        &'a self,
        name: &str,
        implementing_override: Option<&'a str>,
        visited: &mut HashSet<String>,
        result: &mut IndexMap<String, ResolvedMethod<'a>>,
    ) {
        let key = symbol_key(name);
        if !visited.insert(key) {
            return;
        }
        let Some(class_like) = self.class_like(name) else {
            return;
        };
        let implementing_class = implementing_override.unwrap_or(&class_like.name);

        for method in class_like.methods.values() {
            result
                .entry(method.name.to_ascii_lowercase())
                .or_insert_with(|| ResolvedMethod {
                    method,
                    name: method.name.clone(),
                    visibility: method.visibility,
                    declaring_class: &method.declaring_class,
                    implementing_class,
                });
        }

        for trait_use in &class_like.trait_uses {
            let mut candidates: IndexMap<String, ResolvedMethod<'a>> = IndexMap::new();
            for trait_name in &trait_use.traits {
                let mut trait_methods = IndexMap::new();
                self.collect_methods(
                    trait_name,
                    Some(implementing_class),
                    &mut HashSet::new(),
                    &mut trait_methods,
                );
                for (method_key, method) in trait_methods {
                    let suppressed = trait_use.precedences.iter().any(|precedence| {
                        precedence.method.eq_ignore_ascii_case(&method.name)
                            && precedence
                                .instead_of
                                .iter()
                                .any(|excluded| symbol_key(excluded) == symbol_key(trait_name))
                    });
                    if !suppressed {
                        candidates.entry(method_key).or_insert(method);
                    }
                }
            }

            for (method_key, method) in &candidates {
                result.entry(method_key.clone()).or_insert_with(|| method.clone());
            }

            for alias in &trait_use.aliases {
                let method_key = alias.method.to_ascii_lowercase();
                let candidate = candidates.values().find(|candidate| {
                    candidate.name.eq_ignore_ascii_case(&alias.method)
                        && alias
                            .trait_name
                            .as_deref()
                            .is_none_or(|name| symbol_key(candidate.declaring_class) == symbol_key(name))
                });
                let Some(candidate) = candidate else {
                    continue;
                };
                if let Some(alias_name) = &alias.alias {
                    result
                        .entry(alias_name.to_ascii_lowercase())
                        .or_insert_with(|| ResolvedMethod {
                            method: candidate.method,
                            name: alias_name.clone(),
                            visibility: alias.visibility.unwrap_or(candidate.visibility),
                            declaring_class: candidate.declaring_class,
                            implementing_class,
                        });
                } else if let Some(visibility) = alias.visibility
                    && let Some(imported) = result.get_mut(&method_key)
                {
                    imported.visibility = visibility;
                }
            }
        }

        for parent in &class_like.extends {
            self.collect_methods(parent, None, visited, result);
        }
        for interface in &class_like.implements {
            self.collect_methods(interface, None, visited, result);
        }
    }

    fn collect_properties<'a>(
        &'a self,
        name: &str,
        implementing_override: Option<&'a str>,
        visited: &mut HashSet<String>,
        result: &mut IndexMap<String, ResolvedProperty<'a>>,
    ) {
        if !visited.insert(symbol_key(name)) {
            return;
        }
        let Some(class_like) = self.class_like(name) else {
            return;
        };
        let implementing_class = implementing_override.unwrap_or(&class_like.name);
        for property in class_like.properties.values() {
            result.entry(property.name.clone()).or_insert(ResolvedProperty {
                property,
                declaring_class: &property.declaring_class,
                implementing_class,
            });
        }
        for trait_use in &class_like.trait_uses {
            for trait_name in &trait_use.traits {
                self.collect_properties(trait_name, Some(implementing_class), visited, result);
            }
        }
        for parent in &class_like.extends {
            self.collect_properties(parent, None, visited, result);
        }
        for interface in &class_like.implements {
            self.collect_properties(interface, None, visited, result);
        }
    }

    fn collect_constants<'a>(
        &'a self,
        name: &str,
        implementing_override: Option<&'a str>,
        visited: &mut HashSet<String>,
        result: &mut IndexMap<String, ResolvedConstant<'a>>,
    ) {
        if !visited.insert(symbol_key(name)) {
            return;
        }
        let Some(class_like) = self.class_like(name) else {
            return;
        };
        let implementing_class = implementing_override.unwrap_or(&class_like.name);
        for constant in class_like.constants.values() {
            result.entry(constant.name.clone()).or_insert(ResolvedConstant {
                constant,
                declaring_class: &constant.declaring_class,
                implementing_class,
            });
        }
        for trait_use in &class_like.trait_uses {
            for trait_name in &trait_use.traits {
                self.collect_constants(trait_name, Some(implementing_class), visited, result);
            }
        }
        for parent in &class_like.extends {
            self.collect_constants(parent, None, visited, result);
        }
        for interface in &class_like.implements {
            self.collect_constants(interface, None, visited, result);
        }
    }
}

#[must_use]
pub fn symbol_key(name: &str) -> String {
    name.trim_start_matches('\\').to_ascii_lowercase()
}

fn expand_sources(
    root: &Path,
    sources: impl IntoIterator<Item = SourceFile>,
    excludes_project_path: &impl Fn(&Path) -> bool,
) -> Result<Vec<SourceFile>, CheckError> {
    let canonical_root = root.canonicalize()?;
    let mut expanded = Vec::new();
    let mut seen = HashSet::new();
    for source in sources {
        expand_source(
            root,
            &canonical_root,
            source,
            true,
            excludes_project_path,
            &mut seen,
            &mut expanded,
        )?;
    }
    expanded.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(expanded)
}

fn expand_source(
    root: &Path,
    canonical_root: &Path,
    source: SourceFile,
    explicit: bool,
    excludes_project_path: &impl Fn(&Path) -> bool,
    seen: &mut HashSet<PathBuf>,
    expanded: &mut Vec<SourceFile>,
) -> Result<(), CheckError> {
    let identity = match source.path.canonicalize() {
        Ok(identity) => identity,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !identity.starts_with(canonical_root) {
        return Err(CheckError::Extraction(format!(
            "source path escapes the repository checkout: {}",
            source.path.display()
        )));
    }
    let relative_path = source
        .path
        .strip_prefix(root)
        .map(normalize_relative_path)
        .unwrap_or_else(|_| {
            identity
                .strip_prefix(canonical_root)
                .expect("a validated source path is inside the repository root")
                .to_path_buf()
        });
    if source.role == SourceRole::Project && excludes_project_path(&relative_path) {
        return Ok(());
    }
    if identity.is_dir() {
        if !seen.insert(identity) {
            return Ok(());
        }
        let mut entries = fs::read_dir(&source.path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::path);
        for entry in entries {
            expand_source(
                root,
                canonical_root,
                SourceFile {
                    path: entry.path(),
                    role: source.role,
                },
                false,
                excludes_project_path,
                seen,
                expanded,
            )?;
        }
    } else if (explicit || source.path.extension().is_some_and(|extension| extension == "php")) && seen.insert(identity)
    {
        expanded.push(source);
    }
    Ok(())
}

fn normalize_relative_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                unreachable!("a path stripped from the repository root must be relative")
            }
        }
    }
    normalized
}

fn capture_program(
    file: &File,
    logical_path: &Path,
    role: SourceRole,
    program: &Program<'_>,
    resolved_names: &ResolvedNames<'_>,
) -> Vec<ClassLike> {
    let mut class_likes = Vec::new();
    let mut context = CaptureContext {
        file,
        logical_path,
        role,
        program,
        names: resolved_names,
        class_likes: &mut class_likes,
    };
    DeclarationWalker.walk_program(program, &mut context);
    class_likes
}

struct CaptureContext<'ctx, 'ast, 'arena> {
    file: &'ctx File,
    logical_path: &'ctx Path,
    role: SourceRole,
    program: &'ast Program<'arena>,
    names: &'ctx ResolvedNames<'arena>,
    class_likes: &'ctx mut Vec<ClassLike>,
}

#[derive(Debug, Clone, Copy)]
struct DeclarationWalker;

impl<'ast, 'arena> Walker<'ast, 'arena, CaptureContext<'_, 'ast, 'arena>> for DeclarationWalker {
    fn walk_in_class(&self, class: &'ast Class<'arena>, context: &mut CaptureContext<'_, 'ast, 'arena>) {
        context.class_likes.push(capture_class(
            context.file,
            context.logical_path,
            context.role,
            context.program,
            context.names,
            class,
        ));
    }

    fn walk_in_interface(&self, interface: &'ast Interface<'arena>, context: &mut CaptureContext<'_, 'ast, 'arena>) {
        context.class_likes.push(capture_interface(
            context.file,
            context.logical_path,
            context.role,
            context.program,
            context.names,
            interface,
        ));
    }

    fn walk_in_trait(&self, r#trait: &'ast Trait<'arena>, context: &mut CaptureContext<'_, 'ast, 'arena>) {
        context.class_likes.push(capture_trait(
            context.file,
            context.logical_path,
            context.role,
            context.program,
            context.names,
            r#trait,
        ));
    }

    fn walk_in_enum(&self, r#enum: &'ast Enum<'arena>, context: &mut CaptureContext<'_, 'ast, 'arena>) {
        context.class_likes.push(capture_enum(
            context.file,
            context.logical_path,
            context.role,
            context.program,
            context.names,
            r#enum,
        ));
    }
}

fn capture_class(
    file: &File,
    path: &Path,
    role: SourceRole,
    program: &Program<'_>,
    names: &ResolvedNames<'_>,
    class: &Class<'_>,
) -> ClassLike {
    let name = resolved_local_name(&class.name, names);
    let mut result = class_like_base(
        file,
        path,
        role,
        program,
        names,
        name,
        ClassLikeKind::Class,
        class.span(),
        class.modifiers.iter(),
    );
    result.extends = class
        .extends
        .iter()
        .flat_map(|extends| extends.types.iter())
        .map(|name| resolved_name(name, names))
        .collect();
    result.implements = class
        .implements
        .iter()
        .flat_map(|implements| implements.types.iter())
        .map(|name| resolved_name(name, names))
        .collect();
    capture_members(&mut result, class.members.iter(), file, path, program, names);
    result
}

fn capture_interface(
    file: &File,
    path: &Path,
    role: SourceRole,
    program: &Program<'_>,
    names: &ResolvedNames<'_>,
    interface: &Interface<'_>,
) -> ClassLike {
    let name = resolved_local_name(&interface.name, names);
    let mut result = class_like_base(
        file,
        path,
        role,
        program,
        names,
        name,
        ClassLikeKind::Interface,
        interface.span(),
        std::iter::empty(),
    );
    result.is_abstract = true;
    result.extends = interface
        .extends
        .iter()
        .flat_map(|extends| extends.types.iter())
        .map(|name| resolved_name(name, names))
        .collect();
    capture_members(&mut result, interface.members.iter(), file, path, program, names);
    result
}

fn capture_trait(
    file: &File,
    path: &Path,
    role: SourceRole,
    program: &Program<'_>,
    names: &ResolvedNames<'_>,
    r#trait: &Trait<'_>,
) -> ClassLike {
    let name = resolved_local_name(&r#trait.name, names);
    let mut result = class_like_base(
        file,
        path,
        role,
        program,
        names,
        name,
        ClassLikeKind::Trait,
        r#trait.span(),
        std::iter::empty(),
    );
    capture_members(&mut result, r#trait.members.iter(), file, path, program, names);
    result
}

fn capture_enum(
    file: &File,
    path: &Path,
    role: SourceRole,
    program: &Program<'_>,
    names: &ResolvedNames<'_>,
    r#enum: &Enum<'_>,
) -> ClassLike {
    let name = resolved_local_name(&r#enum.name, names);
    let mut result = class_like_base(
        file,
        path,
        role,
        program,
        names,
        name,
        ClassLikeKind::Enum,
        r#enum.span(),
        std::iter::empty(),
    );
    result.is_final = true;
    result.enum_backing_type = r#enum
        .backing_type_hint
        .as_ref()
        .map(|backing| native_type(&backing.hint, names));
    result.implements = r#enum
        .implements
        .iter()
        .flat_map(|implements| implements.types.iter())
        .map(|name| resolved_name(name, names))
        .collect();
    capture_members(&mut result, r#enum.members.iter(), file, path, program, names);
    result
}

#[allow(clippy::too_many_arguments)]
fn class_like_base<'arena>(
    file: &File,
    path: &Path,
    role: SourceRole,
    program: &Program<'arena>,
    _names: &ResolvedNames<'arena>,
    name: String,
    kind: ClassLikeKind,
    span: Span,
    modifiers: impl Iterator<Item = &'arena Modifier<'arena>>,
) -> ClassLike {
    let modifiers = modifiers.collect::<Vec<_>>();
    let docblock = docblock(program, span);
    ClassLike {
        name,
        kind,
        role,
        location: location(file, path, span),
        is_internal: docblock.as_deref().is_some_and(has_internal_annotation),
        docblock,
        is_final: modifiers.iter().any(|modifier| modifier.is_final()),
        is_abstract: modifiers.iter().any(|modifier| modifier.is_abstract()),
        is_readonly: modifiers.iter().any(|modifier| modifier.is_readonly()),
        extends: Vec::new(),
        implements: Vec::new(),
        trait_uses: Vec::new(),
        enum_backing_type: None,
        methods: IndexMap::new(),
        properties: IndexMap::new(),
        constants: IndexMap::new(),
        enum_cases: IndexMap::new(),
    }
}

fn capture_members<'arena>(
    class_like: &mut ClassLike,
    members: impl Iterator<Item = &'arena ClassLikeMember<'arena>>,
    file: &File,
    path: &Path,
    program: &Program<'arena>,
    names: &ResolvedNames<'arena>,
) {
    for member in members {
        match member {
            ClassLikeMember::TraitUse(r#use) => class_like.trait_uses.push(capture_trait_use(r#use, names)),
            ClassLikeMember::Method(method) => {
                let method = capture_method(class_like, method, file, path, program, names);
                if method.name.eq_ignore_ascii_case("__construct") {
                    for parameter in &method.parameters {
                        let Some(visibility) = parameter.promoted_visibility else {
                            continue;
                        };
                        class_like
                            .properties
                            .entry(parameter.name.clone())
                            .or_insert_with(|| Property {
                                name: parameter.name.clone(),
                                declaring_class: class_like.name.clone(),
                                location: parameter.location.clone(),
                                docblock: None,
                                is_internal: false,
                                visibility,
                                is_static: false,
                                is_readonly: parameter.promoted_readonly,
                                native_type: parameter.native_type.clone(),
                                // Constructor defaults initialize parameters, not the promoted
                                // property itself. Reflection reports no property default here.
                                default_value: PhpValue::Null,
                                promoted: true,
                            });
                    }
                }
                class_like.methods.insert(method.name.to_ascii_lowercase(), method);
            }
            ClassLikeMember::Property(property) => {
                for property in capture_property(class_like, property, file, path, program, names) {
                    class_like.properties.insert(property.name.clone(), property);
                }
            }
            ClassLikeMember::Constant(constant) => {
                let visibility = visibility(constant.modifiers.iter());
                let is_final = constant.modifiers.iter().any(Modifier::is_final);
                let native_type = constant.hint.as_ref().map(|hint| native_type(hint, names));
                for item in &constant.items {
                    let name = bytes_string(item.name.value);
                    class_like.constants.insert(
                        name.clone(),
                        ClassConstant {
                            name,
                            declaring_class: class_like.name.clone(),
                            location: location(file, path, constant.span()),
                            visibility,
                            is_final,
                            native_type: native_type.clone(),
                            value: value_from_expression(item.value, file.contents.as_ref(), names),
                        },
                    );
                }
            }
            ClassLikeMember::EnumCase(case) => {
                let docblock = docblock(program, case.span());
                let (name, value) = match &case.item {
                    EnumCaseItem::Unit(item) => (bytes_string(item.name.value), None),
                    EnumCaseItem::Backed(item) => (
                        bytes_string(item.name.value),
                        Some(value_from_expression(item.value, file.contents.as_ref(), names)),
                    ),
                };
                class_like.enum_cases.insert(
                    name.clone(),
                    EnumCase {
                        name,
                        declaring_class: class_like.name.clone(),
                        location: location(file, path, case.span()),
                        is_internal: docblock.as_deref().is_some_and(has_internal_annotation),
                        docblock,
                        value,
                    },
                );
            }
        }
    }
}

fn capture_method(
    class_like: &ClassLike,
    method: &mago_syntax::cst::Method<'_>,
    file: &File,
    path: &Path,
    program: &Program<'_>,
    names: &ResolvedNames<'_>,
) -> Method {
    let docblock = docblock(program, method.span());
    let parameters = method
        .parameter_list
        .parameters
        .iter()
        .enumerate()
        .map(|(position, parameter)| Parameter {
            name: variable_name(parameter.variable.name),
            position,
            location: location(file, path, parameter.span()),
            native_type: parameter.hint.as_ref().map(|hint| native_type(hint, names)),
            by_reference: parameter.ampersand.is_some(),
            variadic: parameter.ellipsis.is_some(),
            has_default: parameter.default_value.is_some(),
            default_value: parameter
                .default_value
                .as_ref()
                .map(|default| value_from_expression(default.value, file.contents.as_ref(), names)),
            promoted_visibility: promoted_visibility(parameter.modifiers.iter()),
            promoted_readonly: parameter.modifiers.iter().any(Modifier::is_readonly),
        })
        .collect();

    Method {
        name: bytes_string(method.name.value),
        declaring_class: class_like.name.clone(),
        location: location(file, path, method.span()),
        is_internal: docblock.as_deref().is_some_and(has_internal_annotation),
        no_named_arguments: docblock
            .as_deref()
            .is_some_and(|docblock| docblock.contains("@no-named-arguments")),
        docblock,
        visibility: visibility(method.modifiers.iter()),
        is_static: method.modifiers.iter().any(Modifier::is_static),
        is_final: method.modifiers.iter().any(Modifier::is_final),
        is_abstract: class_like.kind == ClassLikeKind::Interface
            || method.modifiers.iter().any(Modifier::is_abstract)
            || matches!(method.body, MethodBody::Abstract(_)),
        returns_by_reference: method.ampersand.is_some(),
        parameters,
        return_type: method
            .return_type_hint
            .as_ref()
            .map(|hint| native_type(&hint.hint, names)),
    }
}

fn capture_property(
    class_like: &ClassLike,
    property: &AstProperty<'_>,
    file: &File,
    path: &Path,
    program: &Program<'_>,
    names: &ResolvedNames<'_>,
) -> Vec<Property> {
    let modifiers = property.modifiers();
    let docblock = docblock(program, property.span());
    let make_property = |item: &PropertyItem<'_>| {
        let (variable, default_value) = match item {
            PropertyItem::Abstract(item) => (&item.variable, PhpValue::Null),
            PropertyItem::Concrete(item) => (
                &item.variable,
                value_from_expression(item.value, file.contents.as_ref(), names),
            ),
        };
        Property {
            name: variable_name(variable.name),
            declaring_class: class_like.name.clone(),
            location: location(file, path, property.span()),
            is_internal: docblock.as_deref().is_some_and(has_internal_annotation),
            docblock: docblock.clone(),
            visibility: visibility(modifiers.iter()),
            is_static: modifiers.iter().any(Modifier::is_static),
            is_readonly: modifiers.iter().any(Modifier::is_readonly),
            native_type: property.hint().map(|hint| native_type(hint, names)),
            default_value,
            promoted: false,
        }
    };

    match property {
        AstProperty::Plain(property) => property.items.iter().map(make_property).collect(),
        AstProperty::Hooked(property) => vec![make_property(&property.item)],
    }
}

fn capture_trait_use(r#use: &mago_syntax::cst::TraitUse<'_>, names: &ResolvedNames<'_>) -> TraitUse {
    let mut aliases = Vec::new();
    let mut precedences = Vec::new();
    if let TraitUseSpecification::Concrete(specification) = &r#use.specification {
        for adaptation in &specification.adaptations {
            match adaptation {
                TraitUseAdaptation::Alias(alias) => {
                    let (trait_name, method) = match &alias.method_reference {
                        TraitUseMethodReference::Identifier(method) => (None, bytes_string(method.value)),
                        TraitUseMethodReference::Absolute(method) => (
                            Some(resolved_name(&method.trait_name, names)),
                            bytes_string(method.method_name.value),
                        ),
                    };
                    aliases.push(TraitAlias {
                        trait_name,
                        method,
                        alias: alias.alias.as_ref().map(|alias| bytes_string(alias.value)),
                        visibility: alias.modifier.as_ref().and_then(modifier_visibility),
                    });
                }
                TraitUseAdaptation::Precedence(precedence) => precedences.push(TraitPrecedence {
                    trait_name: resolved_name(&precedence.method_reference.trait_name, names),
                    method: bytes_string(precedence.method_reference.method_name.value),
                    instead_of: precedence
                        .trait_names
                        .iter()
                        .map(|name| resolved_name(name, names))
                        .collect(),
                }),
            }
        }
    }

    TraitUse {
        traits: r#use
            .trait_names
            .iter()
            .map(|name| resolved_name(name, names))
            .collect(),
        aliases,
        precedences,
    }
}

fn native_type(hint: &Hint<'_>, names: &ResolvedNames<'_>) -> NativeType {
    match hint {
        Hint::Identifier(identifier) => NativeType::Named(resolved_name(identifier, names)),
        Hint::Parenthesized(parenthesized) => {
            NativeType::Parenthesized(Box::new(native_type(parenthesized.hint, names)))
        }
        Hint::Nullable(nullable) => NativeType::Nullable(Box::new(native_type(nullable.hint, names))),
        Hint::Union(union) => normalize_union(flatten_union(union.left, union.right, names)),
        Hint::Intersection(intersection) => {
            NativeType::Intersection(flatten_intersection(intersection.left, intersection.right, names))
        }
        Hint::Self_(_) => NativeType::SelfType,
        Hint::Parent(_) => NativeType::ParentType,
        Hint::Static(_) => NativeType::StaticType,
        Hint::Null(_) => NativeType::Named("null".to_owned()),
        Hint::True(_) => NativeType::Named("true".to_owned()),
        Hint::False(_) => NativeType::Named("false".to_owned()),
        Hint::Array(_) => NativeType::Named("array".to_owned()),
        Hint::Callable(_) => NativeType::Named("callable".to_owned()),
        Hint::Void(_) => NativeType::Named("void".to_owned()),
        Hint::Never(_) => NativeType::Named("never".to_owned()),
        Hint::Float(_) => NativeType::Named("float".to_owned()),
        Hint::Bool(_) => NativeType::Named("bool".to_owned()),
        Hint::Integer(_) => NativeType::Named("int".to_owned()),
        Hint::String(_) => NativeType::Named("string".to_owned()),
        Hint::Object(_) => NativeType::Named("object".to_owned()),
        Hint::Mixed(_) => NativeType::Named("mixed".to_owned()),
        Hint::Iterable(_) => NativeType::Named("iterable".to_owned()),
    }
}

fn normalize_union(mut types: Vec<NativeType>) -> NativeType {
    if types.len() == 2
        && let Some(null_index) = types
            .iter()
            .position(|r#type| matches!(r#type, NativeType::Named(name) if name == "null"))
    {
        let other = types.remove(1 - null_index);
        if !matches!(
            other,
            NativeType::Union(_) | NativeType::Intersection(_) | NativeType::Parenthesized(_)
        ) {
            return NativeType::Nullable(Box::new(other));
        }
    }
    NativeType::Union(types)
}

fn flatten_union(left: &Hint<'_>, right: &Hint<'_>, names: &ResolvedNames<'_>) -> Vec<NativeType> {
    let mut result = Vec::new();
    for hint in [left, right] {
        match native_type(hint, names) {
            NativeType::Union(types) => result.extend(types),
            r#type => result.push(r#type),
        }
    }
    result
}

fn flatten_intersection(left: &Hint<'_>, right: &Hint<'_>, names: &ResolvedNames<'_>) -> Vec<NativeType> {
    let mut result = Vec::new();
    for hint in [left, right] {
        match native_type(hint, names) {
            NativeType::Intersection(types) => result.extend(types),
            r#type => result.push(r#type),
        }
    }
    result
}

fn resolved_local_name(identifier: &mago_syntax::cst::LocalIdentifier<'_>, names: &ResolvedNames<'_>) -> String {
    names
        .resolve(&identifier.span.start)
        .map_or_else(|| bytes_string(identifier.value), bytes_string)
        .trim_start_matches('\\')
        .to_owned()
}

fn resolved_name(identifier: &Identifier<'_>, names: &ResolvedNames<'_>) -> String {
    names
        .resolve(&identifier.span().start)
        .map_or_else(|| bytes_string(identifier.value()), bytes_string)
        .trim_start_matches('\\')
        .to_owned()
}

fn visibility<'arena>(mut modifiers: impl Iterator<Item = &'arena Modifier<'arena>>) -> Visibility {
    modifiers.find_map(modifier_visibility).unwrap_or(Visibility::Public)
}

fn promoted_visibility<'arena>(mut modifiers: impl Iterator<Item = &'arena Modifier<'arena>>) -> Option<Visibility> {
    modifiers.find_map(modifier_visibility)
}

fn modifier_visibility(modifier: &Modifier<'_>) -> Option<Visibility> {
    match modifier {
        Modifier::Private(_) => Some(Visibility::Private),
        Modifier::Protected(_) => Some(Visibility::Protected),
        Modifier::Public(_) => Some(Visibility::Public),
        _ => None,
    }
}

fn docblock(program: &Program<'_>, span: Span) -> Option<String> {
    get_docblock_before_position(program.trivia.as_slice(), span.start.offset)
        .map(|trivia| String::from_utf8_lossy(trivia.value).into_owned())
}

fn has_internal_annotation(docblock: &str) -> bool {
    let bytes = docblock.as_bytes();
    let annotation = b"@internal";
    bytes.windows(annotation.len()).enumerate().any(|(index, candidate)| {
        candidate == annotation
            && index > 0
            && index + annotation.len() < bytes.len()
            && bytes[index - 1].is_ascii_whitespace()
            && bytes[index + annotation.len()].is_ascii_whitespace()
    })
}

fn location(file: &File, path: &Path, span: Span) -> SourceLocation {
    SourceLocation {
        path: path.to_path_buf(),
        line: file.line_number(span.start.offset) + 1,
        column: file.column_number(span.start.offset) + 1,
    }
}

fn logical_path(root: &Path, path: &Path) -> PathBuf {
    let relative = path.strip_prefix(root).unwrap_or(path);
    PathBuf::from(relative.to_string_lossy().replace('\\', "/"))
}

fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().replace('\\', "/").into_bytes()
}

fn bytes_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn variable_name(bytes: &[u8]) -> String {
    bytes_string(bytes.strip_prefix(b"$").unwrap_or(bytes))
}

fn write_joined(formatter: &mut fmt::Formatter<'_>, types: &[NativeType], separator: &str) -> fmt::Result {
    for (index, r#type) in types.iter().enumerate() {
        if index > 0 {
            formatter.write_str(separator)?;
        }
        write!(formatter, "{type}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn internal_annotation_matches_upstream_whitespace_rule() {
        assert!(has_internal_annotation("/**\n * @internal\n */"));
        assert!(!has_internal_annotation("/**@internal */"));
        assert!(!has_internal_annotation("/** @internal*/"));
    }

    #[test]
    fn symbol_keys_are_case_insensitive_and_ignore_a_leading_separator() {
        assert_eq!(symbol_key("\\Vendor\\Package\\Thing"), "vendor\\package\\thing");
    }

    #[test]
    fn formats_native_types() {
        let r#type = NativeType::Union(vec![
            NativeType::Named("string".to_owned()),
            NativeType::Intersection(vec![
                NativeType::Named("A".to_owned()),
                NativeType::Named("B".to_owned()),
            ]),
        ]);
        assert_eq!(r#type.to_string(), "string|A&B");
    }

    #[test]
    fn extracts_owned_api_and_materializes_trait_members() {
        let root = tempdir().expect("temporary project");
        let source = root.path().join("src/Api.php");
        fs::create_dir_all(source.parent().expect("source parent")).expect("create source directory");
        fs::write(
            &source,
            r#"<?php
namespace Acme;

trait Named {
    /** @internal */
    public function oldName(string $value = 'x'): int { return 1; }
}

abstract class Api implements \Countable {
    use Named { oldName as protected renamed; }

    public const ANSWER = 101 + 5;
    protected ?string $label = null;

    public function __construct(public readonly int $id = 7) {}
    public function nullable(string|null $value): string|null {}
}

enum State: string {
    /** @internal */
    case Hidden = 'hidden';
    case Ready = 'ready';
}
"#,
        )
        .expect("write PHP fixture");

        let snapshot = Snapshot::build(root.path(), [SourceFile::project(root.path(), &source)])
            .expect("extract PHP declarations");
        let api = snapshot.class_like("Acme\\Api").expect("Api class");
        assert!(api.is_abstract);
        assert_eq!(api.implements, ["Countable"]);
        assert_eq!(api.constants["ANSWER"].value, PhpValue::Integer(106));
        assert_eq!(api.properties["label"].default_value, PhpValue::Null);
        assert!(api.properties["id"].promoted);
        assert_eq!(
            api.methods["nullable"].parameters[0]
                .native_type
                .as_ref()
                .unwrap()
                .to_string(),
            "?string"
        );
        assert_eq!(
            api.methods["nullable"].return_type.as_ref().unwrap().to_string(),
            "?string"
        );
        assert_eq!(api.location.path, PathBuf::from("src/Api.php"));

        let methods = snapshot.methods_of("Acme\\Api");
        assert_eq!(methods["oldname"].declaring_class, "Acme\\Named");
        assert_eq!(methods["oldname"].implementing_class, "Acme\\Api");
        assert_eq!(methods["renamed"].visibility, Visibility::Protected);

        let state = snapshot.class_like("Acme\\State").expect("State enum");
        assert!(state.enum_cases["Hidden"].is_internal);
        assert_eq!(
            state.enum_cases["Ready"].value,
            Some(PhpValue::String(b"ready".to_vec()))
        );
    }

    #[test]
    fn parses_explicit_autoload_files_without_a_php_extension() {
        let root = tempdir().expect("temporary project");
        let source = root.path().join("autoload-file.inc");
        fs::write(&source, "<?php namespace Acme; class Extensionless {}\n").unwrap();

        let snapshot = Snapshot::build(
            root.path(),
            [
                SourceFile::project(root.path(), root.path()),
                SourceFile::project(root.path(), &source),
            ],
        )
        .unwrap();
        assert!(snapshot.class_like("Acme\\Extensionless").is_some());
    }

    #[test]
    fn excludes_project_subtrees_and_explicit_files_but_not_dependencies() {
        let root = tempdir().expect("temporary project");
        let project = root.path().join("src");
        let internal = project.join("Internal");
        let dependency = root.path().join("vendor/acme/package/src");
        fs::create_dir_all(&internal).unwrap();
        fs::create_dir_all(&dependency).unwrap();
        fs::write(project.join("Visible.php"), "<?php namespace Acme; class Visible {}\n").unwrap();
        fs::write(internal.join("Hidden.php"), "<?php namespace Acme; class Hidden {}\n").unwrap();
        fs::write(
            root.path().join("autoload.inc"),
            "<?php namespace Acme; class ExcludedFile {}\n",
        )
        .unwrap();
        fs::write(
            dependency.join("Dependency.php"),
            "<?php namespace Vendor; class Dependency {}\n",
        )
        .unwrap();

        let snapshot = Snapshot::build_with_project_exclusions(
            root.path(),
            [
                SourceFile::project(root.path(), &project),
                SourceFile::project(root.path(), "autoload.inc"),
                SourceFile::dependency(root.path(), &dependency),
            ],
            |path| path.starts_with("src/Internal") || path == Path::new("autoload.inc") || path.starts_with("vendor"),
        )
        .unwrap();

        assert!(snapshot.class_like("Acme\\Visible").is_some());
        assert!(snapshot.class_like("Acme\\Hidden").is_none());
        assert!(snapshot.class_like("Acme\\ExcludedFile").is_none());
        assert_eq!(
            snapshot
                .class_like("Vendor\\Dependency")
                .map(|class_like| class_like.role),
            Some(SourceRole::Dependency)
        );
    }

    #[cfg(unix)]
    #[test]
    fn exclusions_match_the_lexical_path_of_an_in_repository_symlink() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("temporary project");
        fs::create_dir_all(root.path().join("src/Internal")).unwrap();
        fs::create_dir_all(root.path().join("src/Public")).unwrap();
        fs::write(
            root.path().join("src/Public/Api.php"),
            "<?php namespace Acme; class Api {}\n",
        )
        .unwrap();
        symlink("../Public/Api.php", root.path().join("src/Internal/link.php")).unwrap();

        let snapshot = Snapshot::build_with_project_exclusions(
            root.path(),
            [SourceFile::project(root.path(), "src/Internal")],
            |path| path == Path::new("src/Internal/link.php"),
        )
        .unwrap();

        assert!(snapshot.class_like("Acme\\Api").is_none());
    }

    #[test]
    fn materializes_members_inherited_from_php_builtins() {
        let root = tempdir().expect("temporary project");
        let source = root.path().join("api.php");
        fs::write(
            &source,
            r#"<?php
namespace Acme;
class Collection extends \ArrayObject {}
class Failure extends \Exception {}
"#,
        )
        .unwrap();

        let snapshot = Snapshot::build(root.path(), [SourceFile::project(root.path(), source)]).unwrap();

        let methods = snapshot.methods_of("Acme\\Collection");
        assert_eq!(methods["count"].declaring_class, "ArrayObject");
        assert!(methods["count"].method.return_type.is_none());
        let constants = snapshot.constants_of("Acme\\Collection");
        assert_eq!(constants["STD_PROP_LIST"].constant.value, PhpValue::Integer(1));
        let properties = snapshot.properties_of("Acme\\Failure");
        assert_eq!(properties["message"].declaring_class, "Exception");
        assert!(properties["message"].property.native_type.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_nested_source_symlinks_that_escape_the_checkout() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("temporary project");
        let outside = tempdir().expect("outside directory");
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(outside.path().join("Outside.php"), "<?php class Outside {}\n").unwrap();
        symlink(outside.path(), root.path().join("src/linked")).unwrap();

        let error =
            Snapshot::build(root.path(), [SourceFile::project(root.path(), root.path().join("src"))]).unwrap_err();
        assert!(error.to_string().contains("escapes the repository checkout"));
    }
}
