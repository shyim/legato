use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use pcre2::bytes::{Regex, RegexBuilder};
use serde::Deserialize;

use crate::{Change, CheckError, Rule};

pub(crate) const CONFIGURATION_FILENAME: &str = "legato.toml";
pub(crate) const DEFAULT_PHP_VERSION: &str = "8.5.9";
const OLD_CONFIGURATION_FILENAME: &str = "php-bc-check.toml";
const LEGACY_CONFIGURATION_FILENAME: &str = ".roave-backward-compatibility-check.xml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Configuration {
    pub(crate) platform: Platform,
    pub(crate) baseline: Baseline,
    exclusions: ExclusionMatcher,
    pub(crate) filename: Option<PathBuf>,
}

impl Configuration {
    pub(crate) fn load(current_directory: &Path) -> Result<Self, CheckError> {
        let old_filename = current_directory.join(OLD_CONFIGURATION_FILENAME);
        if old_filename.try_exists()? {
            return Err(configuration_error(format!(
                "Configuration `{OLD_CONFIGURATION_FILENAME}` has been renamed. Move it to `{CONFIGURATION_FILENAME}`."
            )));
        }
        let legacy_filename = current_directory.join(LEGACY_CONFIGURATION_FILENAME);
        if legacy_filename.try_exists()? {
            return Err(configuration_error(format!(
                "Legacy configuration `{LEGACY_CONFIGURATION_FILENAME}` is no longer supported. \
                 Migrate it to `{CONFIGURATION_FILENAME}` and remove the XML file."
            )));
        }

        let filename = current_directory.join(CONFIGURATION_FILENAME);
        let contents = match fs::read_to_string(&filename) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::from_file(ConfigurationFile::default(), None);
            }
            Err(error) => return Err(error.into()),
        };

        let parsed: ConfigurationFile = toml::from_str(&contents)
            .map_err(|error| configuration_error(format!("Unable to parse `{CONFIGURATION_FILENAME}`: {error}")))?;

        Self::from_file(parsed, Some(filename))
    }

    fn from_file(parsed: ConfigurationFile, filename: Option<PathBuf>) -> Result<Self, CheckError> {
        Ok(Self {
            platform: Platform::from_file(parsed.platform)?,
            baseline: Baseline::from_file(parsed.baseline)?,
            exclusions: ExclusionMatcher::from_patterns(parsed.paths.exclude)?,
            filename,
        })
    }

    #[must_use]
    pub(crate) fn excludes_project_path(&self, path: &Path) -> bool {
        self.exclusions.is_match(path)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationFile {
    #[serde(default)]
    platform: PlatformFile,
    #[serde(default)]
    paths: PathsFile,
    #[serde(default)]
    baseline: BaselineFile,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PlatformFile {
    php: String,
    extensions: ExtensionPolicy,
}

impl Default for PlatformFile {
    fn default() -> Self {
        Self {
            php: DEFAULT_PHP_VERSION.to_owned(),
            extensions: ExtensionPolicy::All,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ExtensionPolicy {
    #[default]
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Platform {
    php: String,
}

impl Platform {
    fn from_file(file: PlatformFile) -> Result<Self, CheckError> {
        let PlatformFile {
            php,
            extensions: ExtensionPolicy::All,
        } = file;
        let version = semver::Version::parse(&php).map_err(|error| {
            configuration_error(format!(
                "`platform.php` must be an exact semantic version, got `{}`: {error}.",
                php
            ))
        })?;
        if !version.pre.is_empty() || !version.build.is_empty() {
            return Err(configuration_error(format!(
                "`platform.php` must be a stable exact version without pre-release or build metadata, got `{}`.",
                php
            )));
        }
        Ok(Self { php })
    }

    #[must_use]
    pub(crate) fn php(&self) -> &str {
        &self.php
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PathsFile {
    exclude: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct BaselineFile {
    ignored_regex: Vec<String>,
    ignore: Vec<IgnoreRuleFile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct IgnoreRuleFile {
    identifier: Option<Rule>,
    path: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExclusionMatcher {
    patterns: Vec<String>,
    glob_set: GlobSet,
}

impl PartialEq for ExclusionMatcher {
    fn eq(&self, other: &Self) -> bool {
        self.patterns == other.patterns
    }
}

impl Eq for ExclusionMatcher {}

impl ExclusionMatcher {
    fn from_patterns(patterns: Vec<String>) -> Result<Self, CheckError> {
        Self::from_patterns_with_label(patterns, "path exclusion")
    }

    fn from_baseline_pattern(pattern: String) -> Result<Self, CheckError> {
        Self::from_patterns_with_label(vec![pattern], "baseline ignore path")
    }

    fn from_patterns_with_label(patterns: Vec<String>, label: &str) -> Result<Self, CheckError> {
        let mut normalized_patterns = Vec::with_capacity(patterns.len());
        let mut builder = GlobSetBuilder::new();

        for pattern in patterns {
            let normalized = normalize_path_pattern(&pattern, label)?;
            let directory_pattern = normalized.strip_suffix("/**").filter(|pattern| !pattern.is_empty());
            for compiled_pattern in std::iter::once(normalized.as_str()).chain(directory_pattern) {
                let glob = GlobBuilder::new(compiled_pattern)
                    .literal_separator(true)
                    .backslash_escape(false)
                    .build()
                    .map_err(|error| configuration_error(format!("Invalid {label} pattern `{pattern}`: {error}.")))?;
                builder.add(glob);
            }
            normalized_patterns.push(normalized);
        }

        let glob_set = builder
            .build()
            .map_err(|error| configuration_error(format!("Unable to compile {label} patterns: {error}.")))?;

        Ok(Self {
            patterns: normalized_patterns,
            glob_set,
        })
    }

    #[must_use]
    fn is_match(&self, path: &Path) -> bool {
        if path.is_absolute() || path.components().any(|component| component == Component::ParentDir) {
            return false;
        }

        let normalized = path.to_string_lossy().replace('\\', "/");
        let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
        self.glob_set.is_match(normalized)
    }
}

fn normalize_path_pattern(pattern: &str, label: &str) -> Result<String, CheckError> {
    if pattern.is_empty() {
        return Err(configuration_error(format!("{label} patterns cannot be empty.")));
    }

    let mut normalized = pattern.replace('\\', "/");
    let has_windows_prefix = normalized.as_bytes().get(1) == Some(&b':')
        && normalized.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
    if normalized.starts_with('/') || normalized.starts_with("//") || has_windows_prefix {
        return Err(configuration_error(format!(
            "{label} pattern `{pattern}` must be repository-relative."
        )));
    }

    while let Some(remainder) = normalized.strip_prefix("./") {
        normalized = remainder.to_owned();
    }
    if normalized.split('/').any(|component| component == "..") {
        return Err(configuration_error(format!(
            "{label} pattern `{pattern}` must not escape the repository."
        )));
    }
    if normalized.is_empty() {
        return Err(configuration_error(format!("{label} patterns cannot be empty.")));
    }

    if normalized.ends_with('/') {
        normalized.push_str("**");
    }
    Ok(normalized)
}

#[derive(Debug, Clone)]
pub(crate) struct Baseline {
    ignored_changes: Vec<IgnoredChange>,
    ignore_rules: Vec<IgnoreRule>,
}

impl PartialEq for Baseline {
    fn eq(&self, other: &Self) -> bool {
        self.patterns().eq(other.patterns()) && self.ignore_rules == other.ignore_rules
    }
}

impl Eq for Baseline {}

impl Baseline {
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn empty() -> Self {
        Self {
            ignored_changes: Vec::new(),
            ignore_rules: Vec::new(),
        }
    }

    fn from_file(file: BaselineFile) -> Result<Self, CheckError> {
        let ignored_changes = compile_ignored_changes(file.ignored_regex)?;
        let ignore_rules = file
            .ignore
            .into_iter()
            .enumerate()
            .map(|(index, rule)| IgnoreRule::compile(rule, index + 1))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            ignored_changes,
            ignore_rules,
        })
    }

    #[cfg(test)]
    pub(crate) fn from_patterns<I, S>(patterns: I) -> Result<Self, CheckError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let ignored_changes = compile_ignored_changes(patterns)?;

        Ok(Self {
            ignored_changes,
            ignore_rules: Vec::new(),
        })
    }

    #[must_use]
    pub(crate) fn ignores(&self, change: &Change) -> bool {
        if self.ignore_rules.iter().any(|rule| rule.matches(change)) {
            return true;
        }

        let rendered = change.to_string();
        self.ignored_changes
            .iter()
            .any(|ignored| ignored.regex.is_match(rendered.as_bytes()).unwrap_or(false))
    }

    #[must_use]
    pub(crate) fn filter(&self, changes: &[Change]) -> Vec<Change> {
        changes.iter().filter(|change| !self.ignores(change)).cloned().collect()
    }

    pub(crate) fn patterns(&self) -> impl ExactSizeIterator<Item = &str> {
        self.ignored_changes.iter().map(|ignored| ignored.source.as_str())
    }
}

fn compile_ignored_changes<I, S>(patterns: I) -> Result<Vec<IgnoredChange>, CheckError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    patterns
        .into_iter()
        .map(Into::into)
        .map(IgnoredChange::compile)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IgnoreRule {
    identifier: Option<Rule>,
    path: Option<ExclusionMatcher>,
}

impl IgnoreRule {
    fn compile(rule: IgnoreRuleFile, entry: usize) -> Result<Self, CheckError> {
        if rule.identifier.is_none() && rule.path.is_none() {
            return Err(configuration_error(format!(
                "`baseline.ignore` entry {entry} must specify `identifier`, `path`, or both."
            )));
        }

        Ok(Self {
            identifier: rule.identifier,
            path: rule.path.map(ExclusionMatcher::from_baseline_pattern).transpose()?,
        })
    }

    #[must_use]
    fn matches(&self, change: &Change) -> bool {
        let identifier_matches = self
            .identifier
            .as_ref()
            .is_none_or(|identifier| identifier == &change.rule);
        let path_matches = self
            .path
            .as_ref()
            .is_none_or(|matcher| change.source_path().is_some_and(|path| matcher.is_match(path)));

        identifier_matches && path_matches
    }
}

#[derive(Debug, Clone)]
struct IgnoredChange {
    source: String,
    regex: Regex,
}

impl IgnoredChange {
    fn compile(source: String) -> Result<Self, CheckError> {
        let (body, modifiers) = split_php_pattern(&source)?;
        let mut builder = RegexBuilder::new();
        let mut anchored = false;
        let mut inline_modifiers = String::new();

        for modifier in modifiers.chars() {
            match modifier {
                'i' => {
                    builder.caseless(true);
                }
                'm' => {
                    builder.multi_line(true);
                }
                's' => {
                    builder.dotall(true);
                }
                'x' => {
                    builder.extended(true);
                }
                'u' => {
                    builder.utf(true);
                }
                'A' => anchored = true,
                'J' | 'U' | 'n' => inline_modifiers.push(modifier),
                // These PHP compile-time hints/options do not affect ordinary one-line
                // baseline messages. PCRE2's safe Rust builder does not expose them.
                'D' | 'S' | 'X' | 'r' => {}
                _ => {
                    return Err(configuration_error(format!(
                        "Unsupported modifier `{modifier}` in ignored regex `{source}`."
                    )));
                }
            }
        }

        let mut compiled_pattern = String::new();
        if !inline_modifiers.is_empty() {
            write!(compiled_pattern, "(?{inline_modifiers})").expect("writing to a String cannot fail");
        }
        if anchored {
            compiled_pattern.push_str(r"\A(?:");
        }
        compiled_pattern.push_str(body);
        if anchored {
            compiled_pattern.push(')');
        }

        let regex = builder
            .build(&compiled_pattern)
            .map_err(|error| configuration_error(format!("Invalid ignored regex `{source}`: {error}.")))?;

        Ok(Self { source, regex })
    }
}

fn split_php_pattern(source: &str) -> Result<(&str, &str), CheckError> {
    validate_pattern_shape(source)?;
    let bytes = source.as_bytes();
    let mut escaped = false;

    for index in 1..bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'#' => {
                if index == 1 {
                    return Err(configuration_error("Ignored regex patterns cannot be empty."));
                }
                let modifiers = &source[index + 1..];
                if !modifiers.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                    return Err(configuration_error(format!(
                        "Invalid characters after the closing delimiter in ignored regex `{source}`."
                    )));
                }
                return Ok((&source[1..index], modifiers));
            }
            _ => {}
        }
    }

    Err(configuration_error(format!(
        "Ignored regex `{source}` does not have a closing `#` delimiter."
    )))
}

fn validate_pattern_shape(pattern: &str) -> Result<(), CheckError> {
    if !pattern.starts_with('#') {
        return Err(configuration_error(format!(
            "Ignored regex `{pattern}` must use `#` delimiters."
        )));
    }
    if pattern.len() < 3 {
        return Err(configuration_error("Ignored regex patterns cannot be empty."));
    }
    Ok(())
}

fn configuration_error(message: impl Into<String>) -> CheckError {
    CheckError::Configuration(message.into())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pretty_assertions::assert_eq;

    use super::*;

    fn configuration(contents: &str) -> String {
        format!("{contents}\n[platform]\nphp = \"{DEFAULT_PHP_VERSION}\"\nextensions = \"all\"\n")
    }

    fn first_fenced_block<'a>(document: &'a str, language: &str) -> &'a str {
        let opening = format!("```{language}\n");
        document
            .split_once(&opening)
            .and_then(|(_, remainder)| remainder.split_once("\n```"))
            .map(|(block, _)| block)
            .expect("documentation contains the expected fenced block")
    }

    #[test]
    fn documented_configuration_is_valid_and_matches_its_examples() {
        let directory = tempfile::tempdir().unwrap();
        let example = first_fenced_block(include_str!("../docs/CONFIGURATION.md"), "toml");
        fs::write(directory.path().join(CONFIGURATION_FILENAME), example).unwrap();

        let configuration = Configuration::load(directory.path()).unwrap();
        assert_eq!(configuration.platform.php(), "8.5.9");
        assert!(configuration.excludes_project_path(Path::new("src/Generated/Dto.php")));
        assert!(configuration.baseline.ignores(
            &Change::new(Rule::CLASS_REMOVED, "Class Acme\\Legacy has been deleted").in_path("src/Legacy/Api.php")
        ));
        assert!(configuration.baseline.ignores(&Change::new(
            Rule::METHOD_PARAMETER_TYPE_CHANGED,
            "The parameter $id of Acme\\Api#find() changed from string to int"
        )));
    }

    #[test]
    fn uses_defaults_without_a_configuration_file() {
        let directory = tempfile::tempdir().unwrap();
        let configuration = Configuration::load(directory.path()).unwrap();

        assert_eq!(configuration.filename, None);
        assert_eq!(configuration.platform.php(), DEFAULT_PHP_VERSION);
        assert_eq!(configuration.baseline, Baseline::empty());
        assert!(!configuration.excludes_project_path(Path::new("src/Api.php")));
    }

    #[test]
    fn parses_and_applies_toml_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let filename = directory.path().join(CONFIGURATION_FILENAME);
        fs::write(
            &filename,
            configuration(
                r#"[paths]
exclude = ["tests/**", "src/Generated/"]

[baseline]
ignored_regex = [
    '#\[BC\] CHANGED: Method Foo\\Bar\#run\(\)#',
    '#class .* was deleted#i',
]

[[baseline.ignore]]
path = "src/Legacy/**"
"#,
            ),
        )
        .unwrap();

        let configuration = Configuration::load(directory.path()).unwrap();
        assert_eq!(configuration.filename.as_deref(), Some(filename.as_path()));
        assert_eq!(configuration.platform.php(), "8.5.9");
        assert!(configuration.excludes_project_path(Path::new("tests/ApiTest.php")));
        assert!(configuration.excludes_project_path(Path::new("src/Generated/Client.php")));
        assert!(!configuration.excludes_project_path(Path::new("src/Api.php")));
        assert!(configuration.baseline.ignores(&Change::new(
            Rule::METHOD_PARAMETER_TYPE_CHANGED,
            "Method Foo\\Bar#run()"
        )));
        assert!(
            configuration
                .baseline
                .ignores(&Change::new(Rule::CLASS_REMOVED, "Class Example was deleted"))
        );
        assert!(
            !configuration
                .baseline
                .ignores(&Change::new(Rule::METHOD_REMOVED, "Method Example was deleted"))
        );
        assert!(
            configuration.baseline.ignores(
                &Change::new(
                    Rule::METHOD_SCOPE_CHANGED,
                    "The wording is unrelated to the baseline regexes"
                )
                .at(crate::SourceLocation {
                    path: PathBuf::from("src/Legacy/Api.php"),
                    line: 1,
                    column: 1,
                })
            )
        );
        assert!(
            !configuration.baseline.ignores(
                &Change::new(
                    Rule::METHOD_SCOPE_CHANGED,
                    "The wording is unrelated to the baseline regexes"
                )
                .at(crate::SourceLocation {
                    path: PathBuf::from("src/PublicApi.php"),
                    line: 1,
                    column: 1,
                })
            )
        );
    }

    #[test]
    fn accepts_an_empty_configuration_and_empty_sections() {
        for contents in ["", "[platform]\n", "[paths]\n\n[baseline]\n"] {
            let directory = tempfile::tempdir().unwrap();
            fs::write(directory.path().join(CONFIGURATION_FILENAME), contents).unwrap();

            let configuration = Configuration::load(directory.path()).unwrap();
            assert!(configuration.filename.is_some());
            assert_eq!(configuration.platform.php(), DEFAULT_PHP_VERSION);
            assert_eq!(configuration.baseline, Baseline::empty());
        }
    }

    #[test]
    fn fills_in_individual_platform_defaults() {
        for (contents, expected_php) in [
            ("[platform]\nphp = \"8.4.0\"\n", "8.4.0"),
            ("[platform]\nextensions = \"all\"\n", DEFAULT_PHP_VERSION),
        ] {
            let directory = tempfile::tempdir().unwrap();
            fs::write(directory.path().join(CONFIGURATION_FILENAME), contents).unwrap();
            let configuration = Configuration::load(directory.path()).unwrap();
            assert_eq!(configuration.platform.php(), expected_php);
        }
    }

    #[test]
    fn validates_platform_overrides() {
        for (contents, expected) in [
            (
                "[platform]\nphp = \"8.5\"\nextensions = \"all\"\n",
                "exact semantic version",
            ),
            (
                "[platform]\nphp = \"8.5.9-rc.1\"\nextensions = \"all\"\n",
                "stable exact version",
            ),
            (
                "[platform]\nphp = \"8.5.9\"\nextensions = \"configured\"\n",
                "unknown variant",
            ),
            (
                "[platform]\nphp = \"8.5.9\"\nextensions = \"all\"\nunknown = true\n",
                "unknown field",
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            fs::write(directory.path().join(CONFIGURATION_FILENAME), contents).unwrap();
            let error = Configuration::load(directory.path()).unwrap_err().to_string();
            assert!(error.contains(expected), "{contents}\n{error}");
        }
    }

    #[test]
    fn rejects_unknown_fields_at_every_level() {
        for contents in [
            "unknown = true\n",
            "[paths]\nunknown = true\n",
            "[baseline]\nunknown = true\n",
            "[[baseline.ignore]]\nunknown = true\n",
        ] {
            let directory = tempfile::tempdir().unwrap();
            fs::write(directory.path().join(CONFIGURATION_FILENAME), configuration(contents)).unwrap();
            let error = Configuration::load(directory.path()).unwrap_err();
            assert!(matches!(error, CheckError::Configuration(_)), "{contents}");
        }
    }

    #[test]
    fn rejects_empty_structured_ignore_entries() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(CONFIGURATION_FILENAME),
            configuration("[[baseline.ignore]]\n"),
        )
        .unwrap();

        let error = Configuration::load(directory.path()).unwrap_err();
        let CheckError::Configuration(message) = error else {
            panic!("expected a configuration error");
        };
        assert!(message.contains("baseline.ignore"));
        assert!(message.contains("identifier"));
        assert!(message.contains("path"));
    }

    #[test]
    fn rejects_unknown_change_identifiers() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(CONFIGURATION_FILENAME),
            configuration("[[baseline.ignore]]\nidentifier = \"method.not-a-real-check\"\n"),
        )
        .unwrap();

        let error = Configuration::load(directory.path()).unwrap_err();
        assert!(matches!(error, CheckError::Configuration(_)));
    }

    #[test]
    fn structured_ignore_entries_require_every_supplied_selector() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(CONFIGURATION_FILENAME),
            configuration(
                r#"[[baseline.ignore]]
identifier = "method.parameter-type-changed"
path = "src/Legacy/**"
"#,
            ),
        )
        .unwrap();
        let baseline = Configuration::load(directory.path()).unwrap().baseline;

        let change = |identifier, path| {
            Change::new(identifier, "wording does not affect structured matching").at(crate::SourceLocation {
                path: PathBuf::from(path),
                line: 1,
                column: 1,
            })
        };

        assert!(baseline.ignores(&change(Rule::METHOD_PARAMETER_TYPE_CHANGED, "src/Legacy/Api.php")));
        assert!(!baseline.ignores(&change(Rule::METHOD_REMOVED, "src/Legacy/Api.php")));
        assert!(!baseline.ignores(&change(Rule::METHOD_PARAMETER_TYPE_CHANGED, "src/Api.php")));
    }

    #[test]
    fn structured_ignore_entries_are_disjunctive() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(CONFIGURATION_FILENAME),
            configuration(
                r#"[[baseline.ignore]]
identifier = "method.removed"

[[baseline.ignore]]
path = "src/Generated/**"
"#,
            ),
        )
        .unwrap();
        let baseline = Configuration::load(directory.path()).unwrap().baseline;

        let change = |identifier, path| {
            Change::new(identifier, "description is deliberately unrelated").at(crate::SourceLocation {
                path: PathBuf::from(path),
                line: 1,
                column: 1,
            })
        };

        assert!(baseline.ignores(&change(Rule::METHOD_REMOVED, "src/Api.php")));
        assert!(baseline.ignores(&change(Rule::METHOD_PARAMETER_TYPE_CHANGED, "src/Generated/Api.php")));
        assert!(!baseline.ignores(&change(Rule::METHOD_PARAMETER_TYPE_CHANGED, "src/Api.php")));
    }

    #[test]
    fn structured_path_ignore_uses_a_deleted_symbols_source_path() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(CONFIGURATION_FILENAME),
            configuration("[[baseline.ignore]]\npath = \"src/Removed.php\"\n"),
        )
        .unwrap();
        let baseline = Configuration::load(directory.path()).unwrap().baseline;

        let removed =
            Change::new(Rule::CLASS_REMOVED, "Class Acme\\Removed has been deleted").in_path("src/Removed.php");
        let retained = Change::new(Rule::CLASS_REMOVED, "Class Acme\\Api has been deleted").in_path("src/Api.php");

        assert!(baseline.ignores(&removed));
        assert!(!baseline.ignores(&retained));
    }

    #[test]
    fn validates_structured_ignore_path_patterns() {
        for path in ["", "/tmp/**", "C:/tmp/**", "../outside/**", "src/["] {
            let directory = tempfile::tempdir().unwrap();
            let quoted_path = toml::Value::String(path.to_owned()).to_string();
            fs::write(
                directory.path().join(CONFIGURATION_FILENAME),
                configuration(&format!("[[baseline.ignore]]\npath = {quoted_path}\n")),
            )
            .unwrap();

            let error = Configuration::load(directory.path()).unwrap_err();
            assert!(
                matches!(error, CheckError::Configuration(_)),
                "path should be invalid: {path}"
            );
        }
    }

    #[test]
    fn rejects_legacy_xml_even_when_toml_exists() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(LEGACY_CONFIGURATION_FILENAME),
            "<roave-bc-check />",
        )
        .unwrap();
        fs::write(directory.path().join(CONFIGURATION_FILENAME), "").unwrap();

        let error = Configuration::load(directory.path()).unwrap_err();
        let CheckError::Configuration(message) = error else {
            panic!("expected a configuration error");
        };
        assert!(message.contains(LEGACY_CONFIGURATION_FILENAME));
        assert!(message.contains(CONFIGURATION_FILENAME));
        assert!(message.contains("remove"));
    }

    #[test]
    fn reports_the_renamed_configuration_file() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(OLD_CONFIGURATION_FILENAME),
            "[platform]\nphp = \"8.5.9\"\nextensions = \"all\"\n",
        )
        .unwrap();

        let error = Configuration::load(directory.path()).unwrap_err();
        let CheckError::Configuration(message) = error else {
            panic!("expected a configuration error");
        };
        assert!(message.contains(OLD_CONFIGURATION_FILENAME), "{message}");
        assert!(message.contains(CONFIGURATION_FILENAME), "{message}");
        assert!(message.contains("renamed"), "{message}");
    }

    #[test]
    fn normalizes_and_validates_exclusion_patterns() {
        let matcher = ExclusionMatcher::from_patterns(vec![
            r"tests\fixtures\**".to_owned(),
            "src/Generated/".to_owned(),
            "./cache/**".to_owned(),
        ])
        .unwrap();

        assert!(matcher.is_match(Path::new("tests/fixtures/Api.php")));
        assert!(matcher.is_match(Path::new("tests/fixtures")));
        assert!(matcher.is_match(Path::new("src/Generated")));
        assert!(matcher.is_match(Path::new("src/Generated/Api.php")));
        assert!(matcher.is_match(Path::new("cache/Api.php")));
        assert!(!matcher.is_match(Path::new("src/Api.php")));

        for pattern in ["", "/tmp/**", "C:/tmp/**", "../outside/**", "src/../../outside"] {
            assert!(
                ExclusionMatcher::from_patterns(vec![pattern.to_owned()]).is_err(),
                "pattern should be invalid: {pattern}"
            );
        }
    }

    #[test]
    fn rejects_invalid_globs() {
        let error = ExclusionMatcher::from_patterns(vec!["src/[".to_owned()]).unwrap_err();
        assert!(matches!(error, CheckError::Configuration(_)));
    }

    #[test]
    fn pattern_matching_uses_the_full_rendered_change() {
        let baseline = Baseline::from_patterns([r"#^\[BC\] REMOVED: Class A has been deleted$#"]).unwrap();
        assert!(baseline.ignores(&Change::new(Rule::CLASS_REMOVED, "Class A has been deleted")));
        assert!(!baseline.ignores(&Change::new(Rule::CLASS_REMOVED, "Class A was renamed")));
    }

    #[test]
    fn rejects_invalid_patterns_with_a_configuration_error() {
        for pattern in ["not-delimited", "##", "#unclosed", "#valid#?"] {
            assert!(
                Baseline::from_patterns([pattern]).is_err(),
                "pattern should be invalid: {pattern}"
            );
        }
    }
}
