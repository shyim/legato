//! Typed compatibility-rule definitions shared by Legato detectors and consumers.
//!
//! Every [`Rule`] has one stable string identifier and one immutable
//! [`RuleMetadata`] record. Consumers can enumerate [`Rule::ALL`], parse stable
//! identifiers, or enable the `serde` feature to encode them directly.
//!
//! ```
//! use legato_rules::{ModificationType, Rule, RuleCategory};
//!
//! let rule = Rule::MethodParameterTypeChanged;
//! assert_eq!(rule.identifier(), "method.parameter-type-changed");
//! assert_eq!(rule.category(), RuleCategory::Method);
//! assert_eq!(rule.modification_type(), ModificationType::Changed);
//! assert!(rule.is_breaking());
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::fmt;
use std::str::FromStr;

/// The API surface to which a compatibility rule belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RuleCategory {
    /// Class and shared class-like rules.
    Class,
    /// Interface-specific rules.
    Interface,
    /// Trait-specific rules.
    Trait,
    /// Enum-specific rules.
    Enum,
    /// Class or enum constant rules.
    Constant,
    /// Property rules.
    Property,
    /// Method and signature rules.
    Method,
}

impl RuleCategory {
    /// Every rule category in presentation order.
    pub const ALL: &'static [Self] = &[
        Self::Class,
        Self::Interface,
        Self::Trait,
        Self::Enum,
        Self::Constant,
        Self::Property,
        Self::Method,
    ];

    /// Return the stable lowercase category name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Trait => "trait",
            Self::Enum => "enum",
            Self::Constant => "constant",
            Self::Property => "property",
            Self::Method => "method",
        }
    }
}

impl fmt::Display for RuleCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The kind of source change reported by a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum ModificationType {
    /// An API obligation was added.
    Added,
    /// An existing API declaration changed.
    Changed,
    /// An API declaration or capability was removed.
    Removed,
    /// A safe comparison could not be completed.
    Skipped,
}

impl ModificationType {
    /// Return the stable lowercase machine-readable value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Changed => "changed",
            Self::Removed => "removed",
            Self::Skipped => "skipped",
        }
    }

    /// Return the uppercase label used in human-readable findings.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Added => "ADDED",
            Self::Changed => "CHANGED",
            Self::Removed => "REMOVED",
            Self::Skipped => "SKIPPED",
        }
    }
}

impl fmt::Display for ModificationType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The compatibility impact assigned to a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CompatibilityImpact {
    /// Downstream compatibility may be broken.
    Breaking,
    /// The finding is reported without failing the compatibility check.
    Informational,
}

impl CompatibilityImpact {
    /// Return whether this impact fails a compatibility check.
    #[must_use]
    pub const fn is_breaking(self) -> bool {
        matches!(self, Self::Breaking)
    }
}

/// Stable metadata carried by every compatibility rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuleMetadata {
    /// Stable configuration and machine-output identifier.
    pub identifier: &'static str,
    /// API surface classified by the rule.
    pub category: RuleCategory,
    /// Kind of declaration change reported by the rule.
    pub modification_type: ModificationType,
    /// Compatibility impact assigned to the rule.
    pub impact: CompatibilityImpact,
}

macro_rules! rules {
    ($(
        $variant:ident, $constant:ident => {
            identifier: $identifier:literal,
            category: $category:ident,
            modification: $modification:ident,
            impact: $impact:ident
        }
    ),+ $(,)?) => {
        /// A compatibility rule understood by Legato.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Rule {
            $(
                #[doc = concat!("The stable `", $identifier, "` compatibility rule.")]
                $variant
            ),+
        }

        impl Rule {
            /// Every registered rule in canonical detector/documentation order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            $(
                #[doc = concat!("Compatibility alias for [`Rule::", stringify!($variant), "`].")]
                pub const $constant: Self = Self::$variant;
            )+

            /// Return the stable configuration and machine-output identifier.
            #[must_use]
            pub const fn identifier(self) -> &'static str {
                self.metadata().identifier
            }

            /// Return the stable identifier.
            ///
            /// This is an alias for [`Rule::identifier`].
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                self.identifier()
            }

            /// Return all immutable classification metadata for this rule.
            #[must_use]
            pub const fn metadata(self) -> RuleMetadata {
                match self {
                    $(Self::$variant => RuleMetadata {
                        identifier: $identifier,
                        category: RuleCategory::$category,
                        modification_type: ModificationType::$modification,
                        impact: CompatibilityImpact::$impact,
                    }),+
                }
            }

            /// Return the API category classified by this rule.
            #[must_use]
            pub const fn category(self) -> RuleCategory {
                self.metadata().category
            }

            /// Return the declaration modification type reported by this rule.
            #[must_use]
            pub const fn modification_type(self) -> ModificationType {
                self.metadata().modification_type
            }

            /// Return the compatibility impact assigned to this rule.
            #[must_use]
            pub const fn impact(self) -> CompatibilityImpact {
                self.metadata().impact
            }

            /// Return whether this rule fails a compatibility check.
            #[must_use]
            pub const fn is_breaking(self) -> bool {
                self.impact().is_breaking()
            }
        }

        impl FromStr for Rule {
            type Err = ParseRuleError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($identifier => Ok(Self::$variant),)+
                    _ => Err(ParseRuleError::new(value)),
                }
            }
        }
    };
}

rules! {
    ClassRemoved, CLASS_REMOVED => {
        identifier: "class.removed", category: Class, modification: Removed, impact: Breaking
    },
    ClassBecameAbstract, CLASS_BECAME_ABSTRACT => {
        identifier: "class.became-abstract", category: Class, modification: Changed, impact: Breaking
    },
    ClassBecameInterface, CLASS_BECAME_INTERFACE => {
        identifier: "class.became-interface", category: Class, modification: Changed, impact: Breaking
    },
    ClassBecameTrait, CLASS_BECAME_TRAIT => {
        identifier: "class.became-trait", category: Class, modification: Changed, impact: Breaking
    },
    ClassBecameFinal, CLASS_BECAME_FINAL => {
        identifier: "class.became-final", category: Class, modification: Changed, impact: Breaking
    },
    ClassBecameInternal, CLASS_BECAME_INTERNAL => {
        identifier: "class.became-internal", category: Class, modification: Changed, impact: Breaking
    },
    ClassAncestorRemoved, CLASS_ANCESTOR_REMOVED => {
        identifier: "class.ancestor-removed", category: Class, modification: Removed, impact: Breaking
    },
    ClassBecameEnum, CLASS_BECAME_ENUM => {
        identifier: "class.became-enum", category: Class, modification: Changed, impact: Breaking
    },
    InterfaceBecameClass, INTERFACE_BECAME_CLASS => {
        identifier: "interface.became-class", category: Interface, modification: Changed, impact: Breaking
    },
    InterfaceBecameTrait, INTERFACE_BECAME_TRAIT => {
        identifier: "interface.became-trait", category: Interface, modification: Changed, impact: Breaking
    },
    InterfaceAncestorRemoved, INTERFACE_ANCESTOR_REMOVED => {
        identifier: "interface.ancestor-removed", category: Interface, modification: Removed, impact: Breaking
    },
    InterfaceMethodAdded, INTERFACE_METHOD_ADDED => {
        identifier: "interface.method-added", category: Interface, modification: Added, impact: Breaking
    },
    TraitBecameInterface, TRAIT_BECAME_INTERFACE => {
        identifier: "trait.became-interface", category: Trait, modification: Changed, impact: Breaking
    },
    TraitBecameClass, TRAIT_BECAME_CLASS => {
        identifier: "trait.became-class", category: Trait, modification: Changed, impact: Breaking
    },
    EnumKindChanged, ENUM_KIND_CHANGED => {
        identifier: "enum.kind-changed", category: Enum, modification: Changed, impact: Breaking
    },
    EnumCaseRemoved, ENUM_CASE_REMOVED => {
        identifier: "enum.case-removed", category: Enum, modification: Removed, impact: Breaking
    },
    EnumCaseAdded, ENUM_CASE_ADDED => {
        identifier: "enum.case-added", category: Enum, modification: Added, impact: Breaking
    },
    EnumCaseBecameInternal, ENUM_CASE_BECAME_INTERNAL => {
        identifier: "enum.case-became-internal", category: Enum, modification: Changed, impact: Breaking
    },
    EnumCaseInternalRemoved, ENUM_CASE_INTERNAL_REMOVED => {
        identifier: "enum.case-internal-removed", category: Enum, modification: Changed, impact: Breaking
    },
    ConstantRemoved, CONSTANT_REMOVED => {
        identifier: "constant.removed", category: Constant, modification: Removed, impact: Breaking
    },
    ConstantVisibilityReduced, CONSTANT_VISIBILITY_REDUCED => {
        identifier: "constant.visibility-reduced", category: Constant, modification: Changed, impact: Breaking
    },
    ConstantValueChanged, CONSTANT_VALUE_CHANGED => {
        identifier: "constant.value-changed", category: Constant, modification: Changed, impact: Breaking
    },
    ConstantValueComparisonUnsupported, CONSTANT_VALUE_COMPARISON_UNSUPPORTED => {
        identifier: "constant.value-comparison-unsupported", category: Constant, modification: Skipped, impact: Breaking
    },
    PropertyRemoved, PROPERTY_REMOVED => {
        identifier: "property.removed", category: Property, modification: Removed, impact: Breaking
    },
    PropertyBecameInternal, PROPERTY_BECAME_INTERNAL => {
        identifier: "property.became-internal", category: Property, modification: Changed, impact: Breaking
    },
    PropertyTypeChanged, PROPERTY_TYPE_CHANGED => {
        identifier: "property.type-changed", category: Property, modification: Changed, impact: Breaking
    },
    PropertyDefaultValueChanged, PROPERTY_DEFAULT_VALUE_CHANGED => {
        identifier: "property.default-value-changed", category: Property, modification: Changed, impact: Breaking
    },
    PropertyDefaultValueComparisonUnsupported, PROPERTY_DEFAULT_VALUE_COMPARISON_UNSUPPORTED => {
        identifier: "property.default-value-comparison-unsupported", category: Property, modification: Skipped, impact: Breaking
    },
    PropertyVisibilityReduced, PROPERTY_VISIBILITY_REDUCED => {
        identifier: "property.visibility-reduced", category: Property, modification: Changed, impact: Breaking
    },
    PropertyScopeChanged, PROPERTY_SCOPE_CHANGED => {
        identifier: "property.scope-changed", category: Property, modification: Changed, impact: Breaking
    },
    MethodRemoved, METHOD_REMOVED => {
        identifier: "method.removed", category: Method, modification: Removed, impact: Breaking
    },
    MethodBecameFinal, METHOD_BECAME_FINAL => {
        identifier: "method.became-final", category: Method, modification: Changed, impact: Breaking
    },
    MethodBecameAbstract, METHOD_BECAME_ABSTRACT => {
        identifier: "method.became-abstract", category: Method, modification: Changed, impact: Breaking
    },
    MethodScopeChanged, METHOD_SCOPE_CHANGED => {
        identifier: "method.scope-changed", category: Method, modification: Changed, impact: Breaking
    },
    MethodVisibilityReduced, METHOD_VISIBILITY_REDUCED => {
        identifier: "method.visibility-reduced", category: Method, modification: Changed, impact: Breaking
    },
    MethodParameterAdded, METHOD_PARAMETER_ADDED => {
        identifier: "method.parameter-added", category: Method, modification: Added, impact: Breaking
    },
    MethodBecameInternal, METHOD_BECAME_INTERNAL => {
        identifier: "method.became-internal", category: Method, modification: Changed, impact: Breaking
    },
    MethodParameterReferenceChanged, METHOD_PARAMETER_REFERENCE_CHANGED => {
        identifier: "method.parameter-reference-changed", category: Method, modification: Changed, impact: Breaking
    },
    MethodReturnReferenceChanged, METHOD_RETURN_REFERENCE_CHANGED => {
        identifier: "method.return-reference-changed", category: Method, modification: Changed, impact: Breaking
    },
    MethodRequiredParameterCountIncreased, METHOD_REQUIRED_PARAMETER_COUNT_INCREASED => {
        identifier: "method.required-parameter-count-increased", category: Method, modification: Changed, impact: Breaking
    },
    MethodParameterDefaultValueChanged, METHOD_PARAMETER_DEFAULT_VALUE_CHANGED => {
        identifier: "method.parameter-default-value-changed", category: Method, modification: Changed, impact: Breaking
    },
    MethodParameterDefaultValueComparisonUnsupported, METHOD_PARAMETER_DEFAULT_VALUE_COMPARISON_UNSUPPORTED => {
        identifier: "method.parameter-default-value-comparison-unsupported", category: Method, modification: Skipped, impact: Breaking
    },
    MethodReturnTypeNonCovariant, METHOD_RETURN_TYPE_NON_COVARIANT => {
        identifier: "method.return-type-non-covariant", category: Method, modification: Changed, impact: Breaking
    },
    MethodReturnTypeChanged, METHOD_RETURN_TYPE_CHANGED => {
        identifier: "method.return-type-changed", category: Method, modification: Changed, impact: Breaking
    },
    MethodParameterTypeNonContravariant, METHOD_PARAMETER_TYPE_NON_CONTRAVARIANT => {
        identifier: "method.parameter-type-non-contravariant", category: Method, modification: Changed, impact: Breaking
    },
    MethodParameterTypeChanged, METHOD_PARAMETER_TYPE_CHANGED => {
        identifier: "method.parameter-type-changed", category: Method, modification: Changed, impact: Breaking
    },
    MethodNoNamedArgumentsRemoved, METHOD_NO_NAMED_ARGUMENTS_REMOVED => {
        identifier: "method.no-named-arguments-removed", category: Method, modification: Removed, impact: Breaking
    },
    MethodNoNamedArgumentsAdded, METHOD_NO_NAMED_ARGUMENTS_ADDED => {
        identifier: "method.no-named-arguments-added", category: Method, modification: Added, impact: Breaking
    },
    MethodParameterNameChanged, METHOD_PARAMETER_NAME_CHANGED => {
        identifier: "method.parameter-name-changed", category: Method, modification: Changed, impact: Breaking
    },
}

impl fmt::Display for Rule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.identifier())
    }
}

/// Error returned when a stable rule identifier is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseRuleError {
    identifier: String,
}

impl ParseRuleError {
    fn new(identifier: &str) -> Self {
        Self {
            identifier: identifier.to_owned(),
        }
    }

    /// Return the unrecognized identifier.
    #[must_use]
    pub fn identifier(&self) -> &str {
        &self.identifier
    }
}

impl fmt::Display for ParseRuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Unknown compatibility rule identifier `{}`.",
            self.identifier
        )
    }
}

impl std::error::Error for ParseRuleError {}

#[cfg(feature = "serde")]
impl serde::Serialize for Rule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.identifier())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Rule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let identifier = <String as serde::Deserialize>::deserialize(deserializer)?;
        identifier.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn registry_has_unique_canonical_identifiers_in_every_category() {
        assert_eq!(Rule::ALL.len(), 49);

        let identifiers = Rule::ALL.iter().map(|rule| rule.identifier()).collect::<BTreeSet<_>>();
        assert_eq!(identifiers.len(), Rule::ALL.len());

        let categories = Rule::ALL.iter().map(|rule| rule.category()).collect::<BTreeSet<_>>();
        assert_eq!(categories, RuleCategory::ALL.iter().copied().collect());

        for rule in Rule::ALL {
            assert_eq!(rule.metadata().identifier, rule.identifier());
            assert!(rule.identifier().starts_with(rule.category().as_str()));
            assert_eq!(rule.identifier().parse(), Ok(*rule));
            assert_eq!(rule.to_string(), rule.identifier());
        }
    }

    #[test]
    fn every_current_rule_is_breaking_and_has_fixed_metadata() {
        let actual = Rule::ALL
            .iter()
            .map(|rule| {
                (
                    rule.identifier(),
                    rule.category(),
                    rule.modification_type(),
                    rule.impact(),
                )
            })
            .collect::<Vec<_>>();
        let expected = [
            ("class.removed", RuleCategory::Class, ModificationType::Removed),
            ("class.became-abstract", RuleCategory::Class, ModificationType::Changed),
            ("class.became-interface", RuleCategory::Class, ModificationType::Changed),
            ("class.became-trait", RuleCategory::Class, ModificationType::Changed),
            ("class.became-final", RuleCategory::Class, ModificationType::Changed),
            ("class.became-internal", RuleCategory::Class, ModificationType::Changed),
            ("class.ancestor-removed", RuleCategory::Class, ModificationType::Removed),
            ("class.became-enum", RuleCategory::Class, ModificationType::Changed),
            (
                "interface.became-class",
                RuleCategory::Interface,
                ModificationType::Changed,
            ),
            (
                "interface.became-trait",
                RuleCategory::Interface,
                ModificationType::Changed,
            ),
            (
                "interface.ancestor-removed",
                RuleCategory::Interface,
                ModificationType::Removed,
            ),
            (
                "interface.method-added",
                RuleCategory::Interface,
                ModificationType::Added,
            ),
            ("trait.became-interface", RuleCategory::Trait, ModificationType::Changed),
            ("trait.became-class", RuleCategory::Trait, ModificationType::Changed),
            ("enum.kind-changed", RuleCategory::Enum, ModificationType::Changed),
            ("enum.case-removed", RuleCategory::Enum, ModificationType::Removed),
            ("enum.case-added", RuleCategory::Enum, ModificationType::Added),
            (
                "enum.case-became-internal",
                RuleCategory::Enum,
                ModificationType::Changed,
            ),
            (
                "enum.case-internal-removed",
                RuleCategory::Enum,
                ModificationType::Changed,
            ),
            ("constant.removed", RuleCategory::Constant, ModificationType::Removed),
            (
                "constant.visibility-reduced",
                RuleCategory::Constant,
                ModificationType::Changed,
            ),
            (
                "constant.value-changed",
                RuleCategory::Constant,
                ModificationType::Changed,
            ),
            (
                "constant.value-comparison-unsupported",
                RuleCategory::Constant,
                ModificationType::Skipped,
            ),
            ("property.removed", RuleCategory::Property, ModificationType::Removed),
            (
                "property.became-internal",
                RuleCategory::Property,
                ModificationType::Changed,
            ),
            (
                "property.type-changed",
                RuleCategory::Property,
                ModificationType::Changed,
            ),
            (
                "property.default-value-changed",
                RuleCategory::Property,
                ModificationType::Changed,
            ),
            (
                "property.default-value-comparison-unsupported",
                RuleCategory::Property,
                ModificationType::Skipped,
            ),
            (
                "property.visibility-reduced",
                RuleCategory::Property,
                ModificationType::Changed,
            ),
            (
                "property.scope-changed",
                RuleCategory::Property,
                ModificationType::Changed,
            ),
            ("method.removed", RuleCategory::Method, ModificationType::Removed),
            ("method.became-final", RuleCategory::Method, ModificationType::Changed),
            (
                "method.became-abstract",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
            ("method.scope-changed", RuleCategory::Method, ModificationType::Changed),
            (
                "method.visibility-reduced",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
            ("method.parameter-added", RuleCategory::Method, ModificationType::Added),
            (
                "method.became-internal",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
            (
                "method.parameter-reference-changed",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
            (
                "method.return-reference-changed",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
            (
                "method.required-parameter-count-increased",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
            (
                "method.parameter-default-value-changed",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
            (
                "method.parameter-default-value-comparison-unsupported",
                RuleCategory::Method,
                ModificationType::Skipped,
            ),
            (
                "method.return-type-non-covariant",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
            (
                "method.return-type-changed",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
            (
                "method.parameter-type-non-contravariant",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
            (
                "method.parameter-type-changed",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
            (
                "method.no-named-arguments-removed",
                RuleCategory::Method,
                ModificationType::Removed,
            ),
            (
                "method.no-named-arguments-added",
                RuleCategory::Method,
                ModificationType::Added,
            ),
            (
                "method.parameter-name-changed",
                RuleCategory::Method,
                ModificationType::Changed,
            ),
        ];
        let expected = expected
            .into_iter()
            .map(|(identifier, category, modification_type)| {
                (identifier, category, modification_type, CompatibilityImpact::Breaking)
            })
            .collect::<Vec<_>>();

        assert_eq!(actual, expected);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn every_rule_round_trips_through_serde() {
        for rule in Rule::ALL {
            let encoded = serde_json::to_string(rule).unwrap();
            assert_eq!(encoded, format!("\"{}\"", rule.identifier()));
            assert_eq!(serde_json::from_str::<Rule>(&encoded).unwrap(), *rule);
        }
        assert!(serde_json::from_str::<Rule>("\"method.not-real\"").is_err());
    }
}
