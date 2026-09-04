use std::fmt;
use std::path::{Path, PathBuf};

use legato_rules::Rule;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

/// A source position associated with a compatibility finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceLocation {
    /// Source path in the temporary checkout, or an empty path for embedded symbols.
    pub path: PathBuf,
    /// One-based source line.
    pub line: u32,
    /// One-based source column.
    pub column: u32,
}

/// One compatibility finding emitted by Legato.
///
/// Identifier, modification type, and compatibility impact are derived from
/// [`rule`](Self::rule), so a finding cannot carry contradictory classification
/// fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Typed rule that classified the source change.
    pub rule: Rule,
    /// Human-readable compatibility description.
    pub description: String,
    /// Diagnostic location in the target snapshot when one is available.
    pub location: Option<SourceLocation>,
    source_path: Option<PathBuf>,
}

impl Change {
    /// Create a finding classified by `rule`.
    #[must_use]
    pub fn new(rule: Rule, description: impl Into<String>) -> Self {
        Self {
            rule,
            description: description.into(),
            location: None,
            source_path: None,
        }
    }

    /// Return the modification type defined by the rule registry.
    #[must_use]
    pub const fn modification_type(&self) -> legato_rules::ModificationType {
        self.rule.modification_type()
    }

    /// Return whether the rule is backwards-incompatible.
    #[must_use]
    pub const fn is_breaking(&self) -> bool {
        self.rule.is_breaking()
    }

    /// Return the rule's stable configuration and machine-output identifier.
    #[must_use]
    pub const fn identifier(&self) -> &'static str {
        self.rule.identifier()
    }

    /// Attach a target-snapshot diagnostic location.
    #[must_use]
    pub fn at(mut self, location: SourceLocation) -> Self {
        self.location = Some(location);
        self
    }

    /// Attach `location` only when the finding does not already have one.
    #[must_use]
    pub fn with_fallback_location(mut self, location: Option<&SourceLocation>) -> Self {
        if self.location.is_none() {
            self.location = location.cloned();
        }
        self
    }

    /// Set the repository source path affected by the finding.
    ///
    /// This can differ from the diagnostic location for removed or inherited
    /// declarations.
    #[must_use]
    pub fn in_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.source_path = Some(path.into());
        self
    }

    /// Return the affected repository source path.
    ///
    /// An explicitly assigned source path takes precedence over the diagnostic
    /// location path.
    #[must_use]
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path
            .as_deref()
            .or_else(|| self.location.as_ref().map(|location| location.path.as_path()))
    }

    pub(crate) fn set_source_path(&mut self, path: impl Into<PathBuf>) {
        self.source_path = Some(path.into());
    }
}

impl fmt::Display for Change {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = if self.is_breaking() { "[BC] " } else { "     " };
        write!(
            formatter,
            "{prefix}{}: {}",
            self.modification_type().label(),
            self.description
        )
    }
}

impl Serialize for Change {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Change", 5)?;
        state.serialize_field("identifier", &self.rule)?;
        state.serialize_field("modification_type", &self.modification_type())?;
        state.serialize_field("description", &self.description)?;
        state.serialize_field("is_breaking", &self.is_breaking())?;
        state.serialize_field("location", &self.location)?;
        state.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_rendering_and_serialization_from_the_rule() {
        let change = Change::new(Rule::ClassRemoved, "Class A has been deleted");
        assert_eq!(change.to_string(), "[BC] REMOVED: Class A has been deleted");
        assert_eq!(
            serde_json::to_value(change).unwrap(),
            serde_json::json!({
                "identifier": "class.removed",
                "modification_type": "removed",
                "description": "Class A has been deleted",
                "is_breaking": true,
                "location": null,
            })
        );
    }

    #[test]
    fn every_registered_rule_constructs_a_consistent_change() {
        for rule in Rule::ALL {
            let change = Change::new(*rule, "description");
            assert_eq!(change.rule, *rule);
            assert_eq!(change.identifier(), rule.identifier());
            assert_eq!(change.modification_type(), rule.modification_type());
            assert_eq!(change.is_breaking(), rule.is_breaking());
        }
    }

    #[test]
    fn rule_documentation_lists_every_identifier_once_and_in_canonical_order() {
        let documented = include_str!("../RULES.md")
            .lines()
            .filter_map(|line| line.strip_prefix("| `"))
            .filter_map(|line| line.split_once("` |"))
            .map(|(identifier, _)| identifier)
            .collect::<Vec<_>>();
        let expected = Rule::ALL.iter().map(|rule| rule.identifier()).collect::<Vec<_>>();

        assert_eq!(documented, expected);
    }

    #[test]
    fn explicit_source_path_identifies_the_affected_file() {
        let change = Change::new(Rule::ClassRemoved, "removed")
            .in_path("src/Old.php")
            .at(SourceLocation {
                path: PathBuf::from("src/New.php"),
                line: 1,
                column: 1,
            });
        assert_eq!(change.source_path(), Some(Path::new("src/Old.php")));
    }
}
