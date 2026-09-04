use std::collections::HashSet;
use std::ops::Deref;

use indexmap::IndexMap;

use crate::Rule;
use crate::change::{Change, SourceLocation};
use crate::snapshot::{
    ClassConstant, ClassLike, ClassLikeKind, Method, NativeType, Parameter, Property, Snapshot, SourceRole, Visibility,
    symbol_key,
};
use crate::value::PhpValue;

/// Compare the API declared by the old root package with the target snapshot.
///
/// The order in this function mirrors the checker graph assembled by the
/// upstream executable. It is intentionally not sorted after collection:
/// baseline files and formatter goldens rely on detector order.
pub(crate) fn compare(old: &Snapshot, new: &Snapshot) -> Vec<Change> {
    let mut changes = Vec::new();
    for old_class in old
        .class_likes
        .values()
        .filter(|class_like| class_like.role == SourceRole::Project && !class_like.is_internal)
    {
        let Some(new_class) = new.class_like(&old_class.name) else {
            changes.push(
                Change::new(
                    Rule::CLASS_REMOVED,
                    format!("Class {} has been deleted", old_class.name),
                )
                .in_path(old_class.location.path.clone()),
            );
            continue;
        };

        let first_change = changes.len();
        match old_class.kind {
            ClassLikeKind::Interface => compare_interface(old, new, old_class, new_class, &mut changes),
            ClassLikeKind::Trait => compare_trait(old, new, old_class, new_class, &mut changes),
            ClassLikeKind::Class | ClassLikeKind::Enum => {
                compare_class(old, new, old_class, new_class, &mut changes);
            }
        }
        let source_path = if new_class.role == SourceRole::Project {
            &new_class.location.path
        } else {
            &old_class.location.path
        };
        for change in &mut changes[first_change..] {
            change.set_source_path(source_path.clone());
        }
    }
    changes
}

fn compare_class(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old: &ClassLike,
    new: &ClassLike,
    changes: &mut Vec<Change>,
) {
    let class_location = &new.location;

    // Base class checks.
    if (old.kind == ClassLikeKind::Interface) == (new.kind == ClassLikeKind::Interface)
        && !old.is_abstract
        && new.is_abstract
    {
        push_at(
            changes,
            Change::new(
                Rule::CLASS_BECAME_ABSTRACT,
                format!("Class {} became abstract", old.name),
            ),
            class_location,
        );
    }
    if old.kind != ClassLikeKind::Interface && new.kind == ClassLikeKind::Interface {
        push_at(
            changes,
            Change::new(
                Rule::CLASS_BECAME_INTERFACE,
                format!("Class {} became an interface", old.name),
            ),
            class_location,
        );
    }
    if old.kind != ClassLikeKind::Trait && new.kind == ClassLikeKind::Trait {
        push_at(
            changes,
            Change::new(Rule::CLASS_BECAME_TRAIT, format!("Class {} became a trait", old.name)),
            class_location,
        );
    }
    if !old.is_final && new.is_final {
        push_at(
            changes,
            Change::new(Rule::CLASS_BECAME_FINAL, format!("Class {} became final", old.name)),
            class_location,
        );
    }

    compare_removed_constants(old_snapshot, new_snapshot, old, new, changes);
    compare_removed_properties(old_snapshot, new_snapshot, old, new, changes);
    compare_removed_methods(old_snapshot, new_snapshot, old, new, changes);
    compare_class_ancestors(old_snapshot, new_snapshot, old, new, changes);

    if !old.is_internal && new.is_internal {
        push_at(
            changes,
            Change::new(
                Rule::CLASS_BECAME_INTERNAL,
                format!("{} was marked \"@internal\"", old.name),
            ),
            class_location,
        );
    }
    compare_enum_cases(old, new, changes);

    if !old.is_final {
        compare_changed_constants(old_snapshot, new_snapshot, old, new, ConstantMode::Open, changes);
        compare_changed_properties(old_snapshot, new_snapshot, old, new, PropertyMode::Open, changes);
        compare_changed_methods(old_snapshot, new_snapshot, old, new, MethodMode::Open, changes);
    }
    if old.is_final {
        compare_changed_constants(old_snapshot, new_snapshot, old, new, ConstantMode::Final, changes);
        compare_changed_properties(old_snapshot, new_snapshot, old, new, PropertyMode::Final, changes);
        compare_changed_methods(old_snapshot, new_snapshot, old, new, MethodMode::Final, changes);
    }
}

fn compare_interface(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old: &ClassLike,
    new: &ClassLike,
    changes: &mut Vec<Change>,
) {
    let location = &new.location;
    if matches!(new.kind, ClassLikeKind::Class | ClassLikeKind::Enum) {
        push_at(
            changes,
            Change::new(
                Rule::INTERFACE_BECAME_CLASS,
                format!("Interface {} became a class", old.name),
            ),
            location,
        );
    }
    if new.kind == ClassLikeKind::Trait {
        push_at(
            changes,
            Change::new(
                Rule::INTERFACE_BECAME_TRAIT,
                format!("Interface {} became a trait", old.name),
            ),
            location,
        );
    }

    compare_interface_ancestors(old_snapshot, new_snapshot, old, new, changes);
    compare_added_interface_methods(old_snapshot, new_snapshot, old, new, changes);

    // UseClassBasedChecksOnAnInterface.
    if !old.is_internal && new.is_internal {
        push_at(
            changes,
            Change::new(
                Rule::CLASS_BECAME_INTERNAL,
                format!("{} was marked \"@internal\"", old.name),
            ),
            location,
        );
    }
    compare_removed_constants(old_snapshot, new_snapshot, old, new, changes);
    compare_removed_methods(old_snapshot, new_snapshot, old, new, changes);
    compare_changed_constants(old_snapshot, new_snapshot, old, new, ConstantMode::Interface, changes);
    compare_changed_methods(old_snapshot, new_snapshot, old, new, MethodMode::Interface, changes);
}

fn compare_trait(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old: &ClassLike,
    new: &ClassLike,
    changes: &mut Vec<Change>,
) {
    let location = &new.location;
    if new.kind == ClassLikeKind::Interface {
        // Preserved typo from TraitBecameInterface.
        push_at(
            changes,
            Change::new(
                Rule::TRAIT_BECAME_INTERFACE,
                format!("Interface {} became an interface", old.name),
            ),
            location,
        );
    }
    if matches!(new.kind, ClassLikeKind::Class | ClassLikeKind::Enum) {
        push_at(
            changes,
            Change::new(Rule::TRAIT_BECAME_CLASS, format!("Trait {} became a class", old.name)),
            location,
        );
    }

    // UseClassBasedChecksOnATrait.
    if !old.is_internal && new.is_internal {
        push_at(
            changes,
            Change::new(
                Rule::CLASS_BECAME_INTERNAL,
                format!("{} was marked \"@internal\"", old.name),
            ),
            location,
        );
    }
    compare_removed_constants(old_snapshot, new_snapshot, old, new, changes);
    compare_removed_properties(old_snapshot, new_snapshot, old, new, changes);
    compare_removed_methods(old_snapshot, new_snapshot, old, new, changes);
    compare_changed_properties(old_snapshot, new_snapshot, old, new, PropertyMode::Trait, changes);
    compare_changed_methods(old_snapshot, new_snapshot, old, new, MethodMode::Trait, changes);
}

fn compare_class_ancestors(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old: &ClassLike,
    new: &ClassLike,
    changes: &mut Vec<Change>,
) {
    let old_ancestors = old_snapshot
        .parent_class_names(&old.name)
        .into_iter()
        .chain(interface_names(old_snapshot, &old.name));
    let new_ancestors = new_snapshot
        .parent_class_names(&new.name)
        .into_iter()
        .chain(interface_names(new_snapshot, &new.name))
        .map(|name| symbol_key(&name))
        .collect::<HashSet<_>>();
    let removed = old_ancestors
        .filter(|name| !new_ancestors.contains(&symbol_key(name)))
        .collect::<Vec<_>>();
    if !removed.is_empty() {
        let encoded = serde_json::to_string(&removed).expect("class names are JSON strings");
        push_at(
            changes,
            Change::new(
                Rule::CLASS_ANCESTOR_REMOVED,
                format!("These ancestors of {} have been removed: {encoded}", old.name),
            ),
            &new.location,
        );
    }
}

fn compare_interface_ancestors(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old: &ClassLike,
    new: &ClassLike,
    changes: &mut Vec<Change>,
) {
    let new_interfaces = interface_names(new_snapshot, &new.name)
        .into_iter()
        .map(|name| symbol_key(&name))
        .collect::<HashSet<_>>();
    let removed = interface_names(old_snapshot, &old.name)
        .into_iter()
        .filter(|name| !new_interfaces.contains(&symbol_key(name)))
        .collect::<Vec<_>>();
    if !removed.is_empty() {
        let encoded = serde_json::to_string(&removed).expect("interface names are JSON strings");
        push_at(
            changes,
            Change::new(
                Rule::INTERFACE_ANCESTOR_REMOVED,
                format!("These ancestors of {} have been removed: {encoded}", old.name),
            ),
            &new.location,
        );
    }
}

fn compare_enum_cases(old: &ClassLike, new: &ClassLike, changes: &mut Vec<Change>) {
    if old.kind != ClassLikeKind::Enum && new.kind != ClassLikeKind::Enum {
        return;
    }
    if old.kind != ClassLikeKind::Enum {
        push_at(
            changes,
            Change::new(
                Rule::CLASS_BECAME_ENUM,
                format!("{} {} became enum", old.kind, old.name),
            ),
            &new.location,
        );
        return;
    }
    if new.kind != ClassLikeKind::Enum {
        push_at(
            changes,
            Change::new(
                Rule::ENUM_KIND_CHANGED,
                format!("enum {} became {}", old.name, new.kind),
            ),
            &new.location,
        );
        return;
    }

    for case in old.enum_cases.values() {
        if !case.is_internal && !new.enum_cases.contains_key(&case.name) {
            push_at(
                changes,
                Change::new(
                    Rule::ENUM_CASE_REMOVED,
                    format!("Case {}::{} was removed", old.name, case.name),
                ),
                &new.location,
            );
        }
    }
    for case in new.enum_cases.values() {
        if !case.is_internal && !old.enum_cases.contains_key(&case.name) {
            push_at(
                changes,
                Change::new(
                    Rule::ENUM_CASE_ADDED,
                    format!("Case {}::{} was added", old.name, case.name),
                ),
                &new.location,
            );
        }
    }
    for case in new.enum_cases.values() {
        let Some(old_case) = old.enum_cases.get(&case.name) else {
            continue;
        };
        if case.is_internal && !old_case.is_internal {
            push_at(
                changes,
                Change::new(
                    Rule::ENUM_CASE_BECAME_INTERNAL,
                    format!("Case {}::{} was marked \"@internal\"", old.name, case.name),
                ),
                &new.location,
            );
        }
    }
    for case in new.enum_cases.values() {
        let Some(old_case) = old.enum_cases.get(&case.name) else {
            continue;
        };
        if !case.is_internal && old_case.is_internal {
            push_at(
                changes,
                Change::new(
                    Rule::ENUM_CASE_INTERNAL_REMOVED,
                    format!("Case {}::{} had \"@internal\" removed", old.name, case.name),
                ),
                &new.location,
            );
        }
    }
}

fn compare_removed_constants(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old: &ClassLike,
    new: &ClassLike,
    changes: &mut Vec<Change>,
) {
    let old_constants = all_constants(old_snapshot, old);
    let new_constants = all_constants(new_snapshot, new);
    for (name, constant) in old_constants {
        let remains_accessible = new_constants
            .get(&name)
            .is_some_and(|constant| constant.visibility != Visibility::Private);
        if constant.visibility != Visibility::Private && !remains_accessible {
            push_at(
                changes,
                Change::new(
                    Rule::CONSTANT_REMOVED,
                    format!("Constant {}::{} was removed", old.name, constant.name),
                ),
                &new.location,
            );
        }
    }
}

fn compare_removed_properties(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old: &ClassLike,
    new: &ClassLike,
    changes: &mut Vec<Change>,
) {
    let old_properties = all_properties(old_snapshot, old);
    let new_properties = all_properties(new_snapshot, new);
    for (name, property) in old_properties {
        let accessible = property_is_accessible(&property, old);
        let remains_accessible = new_properties
            .get(&name)
            .is_some_and(|property| property_is_accessible(property, new));
        if accessible && !remains_accessible {
            push_at(
                changes,
                Change::new(
                    Rule::PROPERTY_REMOVED,
                    format!("Property {} was removed", property_name(old_snapshot, old, &property)),
                ),
                &new.location,
            );
        }
    }
}

fn compare_removed_methods(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old: &ClassLike,
    new: &ClassLike,
    changes: &mut Vec<Change>,
) {
    let old_methods = all_methods(old_snapshot, old);
    let new_methods = all_methods(new_snapshot, new);
    for (key, method) in old_methods {
        let accessible = method_is_accessible(&method, old);
        let remains_accessible = new_methods
            .get(&key)
            .is_some_and(|method| method_is_accessible(method, new));
        if accessible && !remains_accessible {
            push_at(
                changes,
                Change::new(
                    Rule::METHOD_REMOVED,
                    format!("Method {} was removed", function_name(&method)),
                ),
                &new.location,
            );
        }
    }
}

fn property_is_accessible(property: &Property, class_like: &ClassLike) -> bool {
    !property.is_internal
        && (property.visibility == Visibility::Public
            || (!class_like.is_final && property.visibility == Visibility::Protected))
}

fn method_is_accessible(method: &Method, class_like: &ClassLike) -> bool {
    !method.is_internal
        && (method.visibility == Visibility::Public
            || (!class_like.is_final && method.visibility == Visibility::Protected))
}

#[derive(Clone, Copy)]
enum ConstantMode {
    Open,
    Final,
    Interface,
}

fn compare_changed_constants(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old_class: &ClassLike,
    new_class: &ClassLike,
    mode: ConstantMode,
    changes: &mut Vec<Change>,
) {
    let old_constants = all_constants(old_snapshot, old_class);
    let new_constants = all_constants(new_snapshot, new_class);
    for (name, old) in old_constants {
        let Some(new) = new_constants.get(&name) else { continue };
        let selected = match mode {
            ConstantMode::Open => old.visibility != Visibility::Private,
            ConstantMode::Final => old.visibility == Visibility::Public,
            ConstantMode::Interface => old.visibility != Visibility::Private,
        };
        if !selected {
            continue;
        }
        if !matches!(mode, ConstantMode::Interface) && old.visibility > new.visibility {
            push_at(
                changes,
                Change::new(
                    Rule::CONSTANT_VISIBILITY_REDUCED,
                    format!(
                        "Constant {}::{} visibility reduced from {} to {}",
                        old.declaring_class, old.name, old.visibility, new.visibility
                    ),
                ),
                &new.location,
            );
        }
        compare_constant_value(&old, new, changes);
    }
}

fn compare_constant_value(old: &ClassConstant, new: &ClassConstant, changes: &mut Vec<Change>) {
    match compare_values(&old.value, &new.value) {
        ValueDifference::Equal => {}
        ValueDifference::Changed => push_at(
            changes,
            Change::new(
                Rule::CONSTANT_VALUE_CHANGED,
                format!(
                    "Value of constant {}::{} changed from {} to {}",
                    old.declaring_class, old.name, old.value, new.value
                ),
            ),
            &new.location,
        ),
        ValueDifference::Unsupported => push_at(
            changes,
            Change::new(
                Rule::CONSTANT_VALUE_COMPARISON_UNSUPPORTED,
                format!(
                    "Unable to compare values of constant {}::{}",
                    old.declaring_class, old.name
                ),
            ),
            &new.location,
        ),
    }
}

#[derive(Clone, Copy)]
enum PropertyMode {
    Open,
    Final,
    Trait,
}

fn compare_changed_properties(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old_class: &ClassLike,
    new_class: &ClassLike,
    mode: PropertyMode,
    changes: &mut Vec<Change>,
) {
    let old_properties = all_properties(old_snapshot, old_class);
    let new_properties = all_properties(new_snapshot, new_class);
    for (name, old) in old_properties {
        let Some(new) = new_properties.get(&name) else { continue };
        let selected = match mode {
            PropertyMode::Open => old.visibility != Visibility::Private && !old.is_internal,
            PropertyMode::Final => old.visibility == Visibility::Public && !old.is_internal,
            PropertyMode::Trait => true,
        };
        if !selected {
            continue;
        }

        let formatted = property_name(old_snapshot, old_class, &old);
        if !old.is_internal && new.is_internal {
            push_at(
                changes,
                Change::new(
                    Rule::PROPERTY_BECAME_INTERNAL,
                    format!("Property {formatted} was marked \"@internal\""),
                ),
                &new.location,
            );
        }
        if !(is_covariant(
            old.native_type.as_ref(),
            new.native_type.as_ref(),
            old_snapshot,
            new_snapshot,
            &old.declaring_class,
            &new.declaring_class,
        ) && is_contravariant(
            old.native_type.as_ref(),
            new.native_type.as_ref(),
            old_snapshot,
            new_snapshot,
            &old.declaring_class,
            &new.declaring_class,
        )) {
            push_at(
                changes,
                Change::new(
                    Rule::PROPERTY_TYPE_CHANGED,
                    format!(
                        "Type of property {formatted} changed from {} to {}",
                        property_type_name(old.native_type.as_ref()),
                        property_type_name(new.native_type.as_ref())
                    ),
                ),
                &new.location,
            );
        }
        match compare_values(&old.default_value, &new.default_value) {
            ValueDifference::Equal => {}
            ValueDifference::Changed => push_at(
                changes,
                Change::new(
                    Rule::PROPERTY_DEFAULT_VALUE_CHANGED,
                    format!(
                        "Property {formatted} changed default value from {} to {}",
                        old.default_value, new.default_value
                    ),
                ),
                &new.location,
            ),
            ValueDifference::Unsupported => push_at(
                changes,
                Change::new(
                    Rule::PROPERTY_DEFAULT_VALUE_COMPARISON_UNSUPPORTED,
                    format!("Unable to compare default value of property {formatted}"),
                ),
                &new.location,
            ),
        }
        if old.visibility > new.visibility {
            push_at(
                changes,
                Change::new(
                    Rule::PROPERTY_VISIBILITY_REDUCED,
                    format!(
                        "Property {formatted} visibility reduced from {} to {}",
                        old.visibility, new.visibility
                    ),
                ),
                &new.location,
            );
        }
        if old.is_static != new.is_static {
            push_at(
                changes,
                Change::new(
                    Rule::PROPERTY_SCOPE_CHANGED,
                    format!(
                        "Property ${} of {} changed scope from {} to {}",
                        old.name,
                        old.declaring_class,
                        scope(old.is_static),
                        scope(new.is_static)
                    ),
                ),
                &new.location,
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MethodMode {
    Open,
    Final,
    Interface,
    Trait,
}

fn compare_changed_methods(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old_class: &ClassLike,
    new_class: &ClassLike,
    mode: MethodMode,
    changes: &mut Vec<Change>,
) {
    let old_methods = all_methods(old_snapshot, old_class);
    let new_methods = all_methods(new_snapshot, new_class);
    for (key, old) in old_methods {
        let Some(new) = new_methods.get(&key) else { continue };
        let selected = match mode {
            MethodMode::Open => old.visibility != Visibility::Private && !old.is_internal,
            MethodMode::Final => old.visibility == Visibility::Public && !old.is_internal,
            MethodMode::Interface | MethodMode::Trait => true,
        };
        if !selected {
            continue;
        }
        compare_method(
            old_snapshot,
            new_snapshot,
            old_class,
            new_class,
            &old,
            new,
            mode,
            changes,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn compare_method(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old_class: &ClassLike,
    new_class: &ClassLike,
    old: &Method,
    new: &Method,
    mode: MethodMode,
    changes: &mut Vec<Change>,
) {
    if !matches!(mode, MethodMode::Interface) {
        if !old.is_final && new.is_final {
            method_rule(
                changes,
                Rule::METHOD_BECAME_FINAL,
                new,
                format!("Method {}() of class {} became final", old.name, old.declaring_class),
            );
        }
        if !old.is_abstract && new.is_abstract {
            method_rule(
                changes,
                Rule::METHOD_BECAME_ABSTRACT,
                new,
                format!(
                    "Method {}() of class {} changed from concrete to abstract",
                    old.name, old.declaring_class
                ),
            );
        }
    }

    if old.is_static != new.is_static {
        method_rule(
            changes,
            Rule::METHOD_SCOPE_CHANGED,
            new,
            format!(
                "Method {}() of class {} changed scope from {} to {}",
                old.name,
                old.declaring_class,
                scope(old.is_static),
                scope(new.is_static)
            ),
        );
    }

    if mode == MethodMode::Final {
        compare_method_parameters_added(old_snapshot, old, new, changes);
    }
    if !matches!(mode, MethodMode::Interface) && old.visibility > new.visibility {
        method_rule(
            changes,
            Rule::METHOD_VISIBILITY_REDUCED,
            new,
            format!(
                "Method {}() of class {} visibility reduced from {} to {}",
                old.name, old.declaring_class, old.visibility, new.visibility
            ),
        );
    }
    if mode != MethodMode::Final {
        compare_method_parameters_added(old_snapshot, old, new, changes);
    }

    compare_function(
        old_snapshot,
        new_snapshot,
        old_class,
        new_class,
        old,
        new,
        mode != MethodMode::Final,
        changes,
    );
}

fn compare_method_parameters_added(old_snapshot: &Snapshot, old: &Method, new: &Method, changes: &mut Vec<Change>) {
    let declaring_is_final = old_snapshot
        .class_like(&old.declaring_class)
        .is_some_and(|class_like| class_like.is_final);
    if declaring_is_final || old.visibility == Visibility::Private {
        return;
    }
    let old_names = old
        .parameters
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect::<HashSet<_>>();
    let required_parameters = required_parameter_count(&new.parameters);
    for parameter in &new.parameters {
        if old.name.eq_ignore_ascii_case("__construct") && parameter.position >= required_parameters {
            continue;
        }
        if !old_names.contains(parameter.name.as_str()) {
            method_rule(
                changes,
                Rule::METHOD_PARAMETER_ADDED,
                new,
                format!(
                    "Parameter {} was added to Method {}() of class {}",
                    parameter.name, old.name, old.declaring_class
                ),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn compare_function(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old_class: &ClassLike,
    new_class: &ClassLike,
    old: &Method,
    new: &Method,
    include_exact_signature_checks: bool,
    changes: &mut Vec<Change>,
) {
    let display_name = function_name(old);
    if !old.is_internal && new.is_internal {
        method_rule(
            changes,
            Rule::METHOD_BECAME_INTERNAL,
            new,
            format!("{display_name} was marked \"@internal\""),
        );
    }

    for (old_parameter, new_parameter) in old.parameters.iter().zip(&new.parameters) {
        if old_parameter.by_reference != new_parameter.by_reference {
            method_rule(
                changes,
                Rule::METHOD_PARAMETER_REFERENCE_CHANGED,
                new,
                format!(
                    "The parameter ${} of {display_name} changed from {} to {}",
                    old_parameter.name,
                    reference(old_parameter.by_reference),
                    reference(new_parameter.by_reference)
                ),
            );
        }
    }
    if old.returns_by_reference != new.returns_by_reference {
        method_rule(
            changes,
            Rule::METHOD_RETURN_REFERENCE_CHANGED,
            new,
            format!(
                "The return value of {display_name} changed from {} to {}",
                reference(old.returns_by_reference),
                reference(new.returns_by_reference)
            ),
        );
    }

    let old_required = required_parameter_count(&old.parameters);
    let new_required = required_parameter_count(&new.parameters);
    if old_required < new_required {
        method_rule(
            changes,
            Rule::METHOD_REQUIRED_PARAMETER_COUNT_INCREASED,
            new,
            format!(
                "The number of required arguments for {display_name} increased from {old_required} to {new_required}"
            ),
        );
    }

    for (position, (old_parameter, new_parameter)) in old.parameters.iter().zip(&new.parameters).enumerate() {
        if !(default_value_available(&old.parameters, position) && default_value_available(&new.parameters, position)) {
            continue;
        }
        let Some(old_value) = old_parameter.default_value.as_ref() else {
            continue;
        };
        let Some(new_value) = new_parameter.default_value.as_ref() else {
            continue;
        };
        match compare_values(old_value, new_value) {
            ValueDifference::Equal => {}
            ValueDifference::Changed => method_rule(
                changes,
                Rule::METHOD_PARAMETER_DEFAULT_VALUE_CHANGED,
                new,
                format!(
                    "Default parameter value for parameter ${} of {display_name} changed from {} to {}",
                    old_parameter.name, old_value, new_value
                ),
            ),
            ValueDifference::Unsupported => method_rule(
                changes,
                Rule::METHOD_PARAMETER_DEFAULT_VALUE_COMPARISON_UNSUPPORTED,
                new,
                format!(
                    "Unable to compare default parameter value for parameter ${} of {display_name}",
                    old_parameter.name
                ),
            ),
        }
    }

    if !is_covariant(
        old.return_type.as_ref(),
        new.return_type.as_ref(),
        old_snapshot,
        new_snapshot,
        &old.declaring_class,
        &new.declaring_class,
    ) {
        method_rule(
            changes,
            Rule::METHOD_RETURN_TYPE_NON_COVARIANT,
            new,
            format!(
                "The return type of {display_name} changed from {} to the non-covariant {}",
                type_name(old.return_type.as_ref()),
                type_name(new.return_type.as_ref())
            ),
        );
    }
    if include_exact_signature_checks && type_name(old.return_type.as_ref()) != type_name(new.return_type.as_ref()) {
        method_rule(
            changes,
            Rule::METHOD_RETURN_TYPE_CHANGED,
            new,
            format!(
                "The return type of {display_name} changed from {} to {}",
                type_name(old.return_type.as_ref()),
                type_name(new.return_type.as_ref())
            ),
        );
    }

    for (old_parameter, new_parameter) in old.parameters.iter().zip(&new.parameters) {
        if !is_contravariant(
            old_parameter.native_type.as_ref(),
            new_parameter.native_type.as_ref(),
            old_snapshot,
            new_snapshot,
            &old.declaring_class,
            &new.declaring_class,
        ) {
            method_rule(
                changes,
                Rule::METHOD_PARAMETER_TYPE_NON_CONTRAVARIANT,
                new,
                format!(
                    "The parameter ${} of {display_name} changed from {} to a non-contravariant {}",
                    old_parameter.name,
                    type_name(old_parameter.native_type.as_ref()),
                    type_name(new_parameter.native_type.as_ref())
                ),
            );
        }
    }
    if include_exact_signature_checks {
        for (old_parameter, new_parameter) in old.parameters.iter().zip(&new.parameters) {
            let old_type = type_name(old_parameter.native_type.as_ref());
            let new_type = type_name(new_parameter.native_type.as_ref());
            if old_type != new_type {
                method_rule(
                    changes,
                    Rule::METHOD_PARAMETER_TYPE_CHANGED,
                    new,
                    format!(
                        "The parameter ${} of {display_name} changed from {old_type} to {new_type}",
                        old_parameter.name
                    ),
                );
            }
        }
        compare_parameter_names(old_snapshot, new_snapshot, old_class, new_class, old, new, changes);
    }
}

#[allow(clippy::too_many_arguments)]
fn compare_parameter_names(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old_class: &ClassLike,
    new_class: &ClassLike,
    old: &Method,
    new: &Method,
    changes: &mut Vec<Change>,
) {
    let old_no_named = has_no_named_arguments(old_snapshot, old_class, old);
    let new_no_named = has_no_named_arguments(new_snapshot, new_class, new);
    let function = function_name(old);
    if old_no_named && !new_no_named {
        method_rule(
            changes,
            Rule::METHOD_NO_NAMED_ARGUMENTS_REMOVED,
            new,
            format!("The @no-named-arguments annotation was removed from {function}"),
        );
        return;
    }
    if !old_no_named && new_no_named {
        method_rule(
            changes,
            Rule::METHOD_NO_NAMED_ARGUMENTS_ADDED,
            new,
            format!("The @no-named-arguments annotation was added from {function}"),
        );
        return;
    }
    if new_no_named {
        return;
    }
    for (old_parameter, new_parameter) in old.parameters.iter().zip(&new.parameters) {
        if old_parameter.name != new_parameter.name {
            method_rule(
                changes,
                Rule::METHOD_PARAMETER_NAME_CHANGED,
                new,
                format!(
                    "Parameter {} of {function} changed name from {} to {}",
                    old_parameter.position, old_parameter.name, new_parameter.name
                ),
            );
        }
    }
}

fn compare_added_interface_methods(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old: &ClassLike,
    new: &ClassLike,
    changes: &mut Vec<Change>,
) {
    let old_methods = all_methods(old_snapshot, old);
    for (key, method) in all_methods(new_snapshot, new) {
        if !old_methods.contains_key(&key) {
            push_at(
                changes,
                Change::new(
                    Rule::INTERFACE_METHOD_ADDED,
                    format!("Method {}() was added to interface {}", method.name, old.name),
                ),
                &new.location,
            );
        }
    }
}

fn has_no_named_arguments(snapshot: &Snapshot, current_class: &ClassLike, method: &Method) -> bool {
    method.no_named_arguments
        || snapshot
            .class_like(&method.declaring_class)
            .unwrap_or(current_class)
            .docblock
            .as_deref()
            .is_some_and(|docblock| docblock.contains("@no-named-arguments"))
}

fn function_name(method: &Method) -> String {
    if method.is_static {
        format!("{}::{}()", method.declaring_class, method.name)
    } else {
        format!("{}#{}()", method.declaring_class, method.name)
    }
}

fn property_name(snapshot: &Snapshot, _implementing_class: &ClassLike, property: &EffectiveProperty) -> String {
    let class_name = snapshot
        .class_like(&property.declaring_class)
        .filter(|class_like| class_like.kind == ClassLikeKind::Trait)
        .map_or(property.declaring_class.as_str(), |_| {
            property.implementing_class.as_str()
        });
    if property.is_static {
        format!("{class_name}::${}", property.name)
    } else {
        format!("{class_name}#${}", property.name)
    }
}

fn type_name(r#type: Option<&NativeType>) -> String {
    r#type.map(ToString::to_string).unwrap_or_else(|| "no type".to_owned())
}

fn property_type_name(r#type: Option<&NativeType>) -> String {
    r#type
        .map(ToString::to_string)
        .unwrap_or_else(|| "having no type".to_owned())
}

const fn scope(is_static: bool) -> &'static str {
    if is_static { "static" } else { "instance" }
}

const fn reference(is_reference: bool) -> &'static str {
    if is_reference { "by-reference" } else { "by-value" }
}

fn required_parameter_count(parameters: &[Parameter]) -> usize {
    parameters
        .iter()
        .rposition(|parameter| !parameter.has_default && !parameter.variadic)
        .map_or(0, |position| position + 1)
}

fn default_value_available(parameters: &[Parameter], position: usize) -> bool {
    parameters
        .get(position)
        .is_some_and(|parameter| parameter.has_default && position >= required_parameter_count(parameters))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueDifference {
    Equal,
    Changed,
    Unsupported,
}

fn compare_values(old: &PhpValue, new: &PhpValue) -> ValueDifference {
    if old.strictly_equals(new) {
        ValueDifference::Equal
    } else if old.is_supported() && new.is_supported() {
        ValueDifference::Changed
    } else {
        ValueDifference::Unsupported
    }
}

fn push_at(changes: &mut Vec<Change>, change: Change, location: &SourceLocation) {
    changes.push(change.at(location.clone()));
}

fn method_rule(changes: &mut Vec<Change>, rule: Rule, method: &Method, description: String) {
    push_at(changes, Change::new(rule, description), &method.location);
}

fn all_methods(snapshot: &Snapshot, class_like: &ClassLike) -> IndexMap<String, Method> {
    snapshot
        .methods_of(&class_like.name)
        .into_iter()
        .map(|(key, resolved)| {
            let mut method = resolved.method.clone();
            method.name = resolved.name;
            method.visibility = resolved.visibility;
            let _ = resolved.implementing_class;
            (key, method)
        })
        .collect()
}

#[derive(Debug, Clone)]
struct EffectiveProperty {
    property: Property,
    implementing_class: String,
}

impl Deref for EffectiveProperty {
    type Target = Property;

    fn deref(&self) -> &Self::Target {
        &self.property
    }
}

fn all_properties(snapshot: &Snapshot, class_like: &ClassLike) -> IndexMap<String, EffectiveProperty> {
    snapshot
        .properties_of(&class_like.name)
        .into_iter()
        .map(|(key, resolved)| {
            let mut property = resolved.property.clone();
            property.declaring_class = resolved.declaring_class.to_owned();
            (
                key,
                EffectiveProperty {
                    property,
                    implementing_class: resolved.implementing_class.to_owned(),
                },
            )
        })
        .collect()
}

fn all_constants(snapshot: &Snapshot, class_like: &ClassLike) -> IndexMap<String, ClassConstant> {
    snapshot
        .constants_of(&class_like.name)
        .into_iter()
        .map(|(key, resolved)| {
            let mut constant = resolved.constant.clone();
            constant.declaring_class = resolved.declaring_class.to_owned();
            let _ = resolved.implementing_class;
            (key, constant)
        })
        .collect()
}

fn interface_names(snapshot: &Snapshot, name: &str) -> Vec<String> {
    fn collect(snapshot: &Snapshot, name: &str, seen: &mut HashSet<String>, interfaces: &mut Vec<String>) {
        let Some(class_like) = snapshot.class_like(name) else {
            return;
        };
        let declared = if class_like.kind == ClassLikeKind::Interface {
            class_like.extends.iter()
        } else {
            class_like.implements.iter()
        };
        for interface in declared {
            if seen.insert(symbol_key(interface)) {
                interfaces.push(interface.clone());
                collect(snapshot, interface, seen, interfaces);
            }
        }
        if class_like.kind == ClassLikeKind::Class {
            for parent in &class_like.extends {
                collect(snapshot, parent, seen, interfaces);
            }
        }
    }

    let mut interfaces = Vec::new();
    collect(snapshot, name, &mut HashSet::new(), &mut interfaces);
    interfaces
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SemanticType {
    Named(String),
    Union(Vec<SemanticType>),
    Intersection(Vec<SemanticType>),
}

fn semantic_type(r#type: &NativeType, snapshot: &Snapshot, declaring_class: &str) -> SemanticType {
    match r#type {
        NativeType::Named(name) => SemanticType::Named(name.clone()),
        NativeType::SelfType | NativeType::StaticType => SemanticType::Named(declaring_class.to_owned()),
        NativeType::ParentType => SemanticType::Named(
            snapshot
                .parent_class_names(declaring_class)
                .into_iter()
                .next()
                .unwrap_or_else(|| "parent".to_owned()),
        ),
        // BetterReflection exposes nullable named types as the underlying name
        // plus an `allowsNull` bit. The upstream variance checks compare only
        // that name; exact signature checks still use NativeType's `?T` display.
        NativeType::Nullable(inner) => semantic_type(inner, snapshot, declaring_class),
        NativeType::Union(types) => SemanticType::Union(
            types
                .iter()
                .map(|r#type| semantic_type(r#type, snapshot, declaring_class))
                .collect(),
        ),
        NativeType::Intersection(types) => SemanticType::Intersection(
            types
                .iter()
                .map(|r#type| semantic_type(r#type, snapshot, declaring_class))
                .collect(),
        ),
        NativeType::Parenthesized(inner) => semantic_type(inner, snapshot, declaring_class),
    }
}

fn is_covariant(
    old: Option<&NativeType>,
    new: Option<&NativeType>,
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old_declaring_class: &str,
    new_declaring_class: &str,
) -> bool {
    let Some(old) = old else {
        return true;
    };
    let Some(new) = new else {
        return false;
    };
    let old = semantic_type(old, old_snapshot, old_declaring_class);
    let new = semantic_type(new, new_snapshot, new_declaring_class);
    semantic_covariant(&old, &new, old_snapshot, new_snapshot)
}

fn semantic_covariant(
    old: &SemanticType,
    new: &SemanticType,
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
) -> bool {
    match (old, new) {
        (_, SemanticType::Union(types)) => types
            .iter()
            .all(|new| semantic_covariant(old, new, old_snapshot, new_snapshot)),
        (SemanticType::Union(types), _) => types
            .iter()
            .any(|old| semantic_covariant(old, new, old_snapshot, new_snapshot)),
        (SemanticType::Intersection(types), _) => types
            .iter()
            .all(|old| semantic_covariant(old, new, old_snapshot, new_snapshot)),
        (_, SemanticType::Intersection(types)) => types
            .iter()
            .any(|new| semantic_covariant(old, new, old_snapshot, new_snapshot)),
        (SemanticType::Named(old), SemanticType::Named(new)) => named_covariant(old, new, old_snapshot, new_snapshot),
    }
}

fn named_covariant(old: &str, new: &str, old_snapshot: &Snapshot, new_snapshot: &Snapshot) -> bool {
    if old.eq_ignore_ascii_case(new) {
        return true;
    }
    if old.eq_ignore_ascii_case("mixed") || new.eq_ignore_ascii_case("never") {
        return true;
    }
    let old_builtin = is_builtin(old);
    let new_builtin = is_builtin(new);
    if old.eq_ignore_ascii_case("object") && !new_builtin {
        return true;
    }
    if new.eq_ignore_ascii_case("array") && old.eq_ignore_ascii_case("iterable") {
        return true;
    }
    if old.eq_ignore_ascii_case("iterable") && !new_builtin && implements_traversable(new_snapshot, new) {
        return true;
    }
    if old_builtin != new_builtin {
        return false;
    }
    if old_builtin {
        return false;
    }
    let old_is_interface = old_snapshot.is_interface(old);
    if old_is_interface {
        return new_snapshot.is_instance_of(new, old);
    }
    new_snapshot.extends_class(new, old)
}

fn is_contravariant(
    old: Option<&NativeType>,
    new: Option<&NativeType>,
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    old_declaring_class: &str,
    new_declaring_class: &str,
) -> bool {
    if old.is_some_and(|r#type| r#type.to_string() == "never")
        || new.is_some_and(|r#type| r#type.to_string() == "never")
    {
        return false;
    }
    if new.is_none() || new.is_some_and(|r#type| r#type.to_string() == "mixed") {
        return true;
    }
    let Some(old) = old else {
        return false;
    };
    let new = new.expect("the no-type case returned above");
    let old = semantic_type(old, old_snapshot, old_declaring_class);
    let new = semantic_type(new, new_snapshot, new_declaring_class);
    semantic_contravariant(&old, &new, old_snapshot, new_snapshot)
}

fn semantic_contravariant(
    old: &SemanticType,
    new: &SemanticType,
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
) -> bool {
    match (old, new) {
        (SemanticType::Union(types), _) => types
            .iter()
            .all(|old| semantic_contravariant(old, new, old_snapshot, new_snapshot)),
        (_, SemanticType::Union(types)) => types
            .iter()
            .any(|new| semantic_contravariant(old, new, old_snapshot, new_snapshot)),
        (_, SemanticType::Intersection(types)) => types
            .iter()
            .all(|new| semantic_contravariant(old, new, old_snapshot, new_snapshot)),
        (SemanticType::Intersection(types), _) => types
            .iter()
            .any(|old| semantic_contravariant(old, new, old_snapshot, new_snapshot)),
        (SemanticType::Named(old), SemanticType::Named(new)) => {
            named_contravariant(old, new, old_snapshot, new_snapshot)
        }
    }
}

fn named_contravariant(old: &str, new: &str, old_snapshot: &Snapshot, new_snapshot: &Snapshot) -> bool {
    if old.eq_ignore_ascii_case(new) {
        return true;
    }
    if old.eq_ignore_ascii_case("void") {
        return true;
    }
    let old_builtin = is_builtin(old);
    let new_builtin = is_builtin(new);
    if new.eq_ignore_ascii_case("object") && !old_builtin {
        return true;
    }
    if new.eq_ignore_ascii_case("iterable") && old.eq_ignore_ascii_case("array") {
        return true;
    }
    if old_builtin != new_builtin {
        return false;
    }
    if old_builtin {
        return false;
    }
    let new_is_interface = new_snapshot.is_interface(new);
    if new_is_interface {
        return old_snapshot.is_instance_of(old, new);
    }
    old_snapshot.extends_class(old, new)
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "array"
            | "bool"
            | "callable"
            | "false"
            | "float"
            | "int"
            | "iterable"
            | "mixed"
            | "never"
            | "null"
            | "object"
            | "string"
            | "true"
            | "void"
    )
}

fn implements_traversable(snapshot: &Snapshot, name: &str) -> bool {
    snapshot.implements_interface(name, "Traversable")
        || matches!(
            symbol_key(name).as_str(),
            "iterator" | "iteratoraggregate" | "generator" | "internaliterator"
        )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::snapshot::SourceFile;

    fn snapshot(source: &str) -> (tempfile::TempDir, Snapshot) {
        let root = tempdir().expect("temporary PHP project");
        let path = root.path().join("api.php");
        fs::write(&path, source).expect("write PHP source");
        let snapshot =
            Snapshot::build(root.path(), [SourceFile::project(root.path(), path)]).expect("build PHP API snapshot");
        (root, snapshot)
    }

    #[test]
    fn every_registered_rule_has_a_behavioral_fixture() {
        let (_old_root, old) = snapshot(
            r#"<?php
namespace Coverage;

interface Contract {}
interface ParentContract {}
class ParentType {}
class ChildType extends ParentType {}

class Removed {}
class BecomesAbstract {}
class BecomesInterface {}
class BecomesTrait {}
class BecomesFinal {}
class BecomesInternal {}
class LosesAncestor extends ParentType implements Contract {}
class BecomesEnum {}

interface InterfaceBecomesClass {}
interface InterfaceBecomesTrait {}
interface LosesInterfaceAncestor extends ParentContract {}
interface InterfaceGainsMethod {}

trait TraitBecomesInterface {}
trait TraitBecomesClass {}

enum EnumBecomesClass {}
enum Cases {
    case Removed;
    case Internalized;
    /** @internal */
    case Publicized;
}

class Members {
    public const REMOVED = 1;
    public const VISIBILITY = 1;
    public const VALUE = 1;
    public const OPAQUE = OLD_CONSTANT_VALUE;

    public string $removed;
    public string $internalized;
    public string $typed;
    public int $defaultChanged = 1;
    public $opaqueDefault = OLD_PROPERTY_VALUE;
    public string $visibility;
    public string $scope;

    public function removed(): void {}
    public function finalized(): void {}
    public function abstracted(): void {}
    public function scoped(): void {}
    public function visibilityReduced(): void {}
    public function parameterAdded(string $id): void {}
    public function internalized(): void {}
    public function parameterReference(string $value): void {}
    public function returnReference(): string {}
    public function requiredCount(?string $value = null): void {}
    public function defaultChanged(int $value = 1): void {}
    public function opaqueDefault($value = OLD_PARAMETER_VALUE): void {}
    public function returnType(): ChildType {}
    public function parameterType(ParentType $value): void {}
    /** @no-named-arguments */
    public function namedArgumentsRemoved($value): void {}
    public function namedArgumentsAdded($value): void {}
    public function parameterRenamed($before): void {}
}
"#,
        );
        let (_new_root, new) = snapshot(
            r#"<?php
namespace Coverage;

interface Contract {}
interface ParentContract {}
class ParentType {}
class ChildType extends ParentType {}

abstract class BecomesAbstract {}
interface BecomesInterface {}
trait BecomesTrait {}
final class BecomesFinal {}
/** @internal */
class BecomesInternal {}
class LosesAncestor {}
enum BecomesEnum {}

class InterfaceBecomesClass {}
trait InterfaceBecomesTrait {}
interface LosesInterfaceAncestor {}
interface InterfaceGainsMethod { public function added(): void; }

interface TraitBecomesInterface {}
class TraitBecomesClass {}

class EnumBecomesClass {}
enum Cases {
    case Added;
    /** @internal */
    case Internalized;
    case Publicized;
}

abstract class Members {
    protected const VISIBILITY = 1;
    public const VALUE = 2;
    public const OPAQUE = NEW_CONSTANT_VALUE;

    /** @internal */
    public string $internalized;
    public int $typed;
    public int $defaultChanged = 2;
    public $opaqueDefault = NEW_PROPERTY_VALUE;
    protected string $visibility;
    public static string $scope;

    final public function finalized(): void {}
    abstract public function abstracted(): void;
    public static function scoped(): void {}
    protected function visibilityReduced(): void {}
    public function parameterAdded(string $id, bool $force = false): void {}
    /** @internal */
    public function internalized(): void {}
    public function parameterReference(string &$value): void {}
    public function &returnReference(): string {}
    public function requiredCount(?string $value): void {}
    public function defaultChanged(int $value = 2): void {}
    public function opaqueDefault($value = NEW_PARAMETER_VALUE): void {}
    public function returnType(): ParentType {}
    public function parameterType(ChildType $value): void {}
    public function namedArgumentsRemoved($value): void {}
    /** @no-named-arguments */
    public function namedArgumentsAdded($value): void {}
    public function parameterRenamed($after): void {}
}
"#,
        );

        let actual = compare(&old, &new)
            .into_iter()
            .map(|change| change.rule)
            .collect::<BTreeSet<_>>();
        let expected = Rule::ALL.iter().copied().collect::<BTreeSet<_>>();
        let missing = expected
            .difference(&actual)
            .map(|rule| rule.identifier())
            .collect::<Vec<_>>();

        assert_eq!(actual, expected, "missing behavioral fixtures for {missing:?}");
    }

    #[test]
    fn preserves_open_class_detector_order_and_messages() {
        let (_old_root, old) = snapshot(
            r#"<?php
namespace Test;
class A {}
class Api {
    public const VALUE = 1;
    public string $name = 'old';
    public function run(A $input = 1): A {}
}
"#,
        );
        let (_new_root, new) = snapshot(
            r#"<?php
namespace Test;
class A {}
class B {}
final class Api {
    protected const VALUE = 2;
    protected int $name = 2;
    final protected static function run(B &$renamed = 2): B {}
}
"#,
        );

        let changes = compare(&old, &new);
        assert_eq!(
            changes.iter().map(Change::identifier).collect::<Vec<_>>(),
            [
                "class.became-final",
                "property.removed",
                "method.removed",
                "constant.visibility-reduced",
                "constant.value-changed",
                "property.type-changed",
                "property.default-value-changed",
                "property.visibility-reduced",
                "method.became-final",
                "method.scope-changed",
                "method.visibility-reduced",
                "method.parameter-added",
                "method.parameter-reference-changed",
                "method.parameter-default-value-changed",
                "method.return-type-non-covariant",
                "method.return-type-changed",
                "method.parameter-type-non-contravariant",
                "method.parameter-type-changed",
                "method.parameter-name-changed",
            ]
        );
        let rendered = changes.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            rendered,
            [
                "[BC] CHANGED: Class Test\\Api became final",
                "[BC] REMOVED: Property Test\\Api#$name was removed",
                "[BC] REMOVED: Method Test\\Api#run() was removed",
                "[BC] CHANGED: Constant Test\\Api::VALUE visibility reduced from public to protected",
                "[BC] CHANGED: Value of constant Test\\Api::VALUE changed from 1 to 2",
                "[BC] CHANGED: Type of property Test\\Api#$name changed from string to int",
                "[BC] CHANGED: Property Test\\Api#$name changed default value from 'old' to 2",
                "[BC] CHANGED: Property Test\\Api#$name visibility reduced from public to protected",
                "[BC] CHANGED: Method run() of class Test\\Api became final",
                "[BC] CHANGED: Method run() of class Test\\Api changed scope from instance to static",
                "[BC] CHANGED: Method run() of class Test\\Api visibility reduced from public to protected",
                "[BC] ADDED: Parameter renamed was added to Method run() of class Test\\Api",
                "[BC] CHANGED: The parameter $input of Test\\Api#run() changed from by-value to by-reference",
                "[BC] CHANGED: Default parameter value for parameter $input of Test\\Api#run() changed from 1 to 2",
                "[BC] CHANGED: The return type of Test\\Api#run() changed from Test\\A to the non-covariant Test\\B",
                "[BC] CHANGED: The return type of Test\\Api#run() changed from Test\\A to Test\\B",
                "[BC] CHANGED: The parameter $input of Test\\Api#run() changed from Test\\A to a non-contravariant Test\\B",
                "[BC] CHANGED: The parameter $input of Test\\Api#run() changed from Test\\A to Test\\B",
                "[BC] CHANGED: Parameter 0 of Test\\Api#run() changed name from input to renamed",
            ]
        );
    }

    #[test]
    fn removed_classes_keep_their_identifier_and_old_source_path() {
        let (_old_root, old) = snapshot("<?php namespace Test; class Removed {}\n");
        let (_new_root, new) = snapshot("<?php\n");

        let changes = compare(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].rule, Rule::CLASS_REMOVED);
        assert_eq!(changes[0].source_path(), Some(std::path::Path::new("api.php")));
        assert!(changes[0].location.is_none());
    }

    #[test]
    fn final_classes_use_only_variance_signature_checks() {
        let (_old_root, old) = snapshot(
            r#"<?php
namespace Test;
class A {}
final class Api { public function run(A $input): A {} }
"#,
        );
        let (_new_root, new) = snapshot(
            r#"<?php
namespace Test;
class A {}
class B {}
final class Api { public function run(B $renamed): B {} }
"#,
        );

        let rendered = compare(&old, &new)
            .into_iter()
            .map(|change| change.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            [
                "[BC] CHANGED: The return type of Test\\Api#run() changed from Test\\A to the non-covariant Test\\B",
                "[BC] CHANGED: The parameter $input of Test\\Api#run() changed from Test\\A to a non-contravariant Test\\B",
            ]
        );
    }

    #[test]
    fn nullable_named_types_only_trigger_exact_signature_checks() {
        let (_old_root, old) = snapshot(
            r#"<?php
namespace Test;
class A {}
class Api {
    public A $value;
    public function run(A $input): A {}
}
"#,
        );
        let (_new_root, new) = snapshot(
            r#"<?php
namespace Test;
class A {}
class Api {
    public A|null $value;
    public function run(A|null $input): A|null {}
}
"#,
        );

        let rendered = compare(&old, &new)
            .into_iter()
            .map(|change| change.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            [
                "[BC] CHANGED: The return type of Test\\Api#run() changed from Test\\A to ?Test\\A",
                "[BC] CHANGED: The parameter $input of Test\\Api#run() changed from Test\\A to ?Test\\A",
            ]
        );
    }

    #[test]
    fn builtin_interfaces_participate_in_variance_checks() {
        let (_old_root, old) = snapshot(
            r#"<?php
class Api {
    public function make(): Traversable {}
    public function take(Iterator $value): void {}
}
"#,
        );
        let (_new_root, new) = snapshot(
            r#"<?php
class Api {
    public function make(): Iterator {}
    public function take(Traversable $value): void {}
}
"#,
        );

        let rendered = compare(&old, &new)
            .into_iter()
            .map(|change| change.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            [
                "[BC] CHANGED: The return type of Api#make() changed from Traversable to Iterator",
                "[BC] CHANGED: The parameter $value of Api#take() changed from Iterator to Traversable",
            ]
        );
    }

    #[test]
    fn compares_overrides_with_members_inherited_from_php_builtins() {
        let (_old_root, old) = snapshot(
            r#"<?php
namespace Test;
class Collection extends \ArrayObject {}
class Failure extends \Exception {}
"#,
        );
        let (_new_root, new) = snapshot(
            r#"<?php
namespace Test;
class Collection extends \ArrayObject {
    public const STD_PROP_LIST = 2;
    public function count(): int { return parent::count(); }
}
class Failure extends \Exception {
    protected string $message = '';
}
"#,
        );

        let rendered = compare(&old, &new)
            .into_iter()
            .map(|change| change.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            [
                "[BC] CHANGED: Value of constant ArrayObject::STD_PROP_LIST changed from 1 to 2",
                "[BC] CHANGED: The return type of ArrayObject#count() changed from no type to int",
                "[BC] CHANGED: Type of property Exception#$message changed from having no type to string",
            ]
        );
    }

    #[test]
    fn compares_removed_overrides_with_the_inherited_builtin_method() {
        let (_old_root, old) = snapshot(
            r#"<?php
namespace Test;
class Collection extends \ArrayObject {
    public function count(): int { return parent::count(); }
}
"#,
        );
        let (_new_root, new) = snapshot(
            r#"<?php
namespace Test;
class Collection extends \ArrayObject {}
"#,
        );

        let changes = compare(&old, &new);
        assert_eq!(
            changes.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "[BC] CHANGED: The return type of Test\\Collection#count() changed from int to the non-covariant no type",
                "[BC] CHANGED: The return type of Test\\Collection#count() changed from int to no type",
            ]
        );
        assert!(changes.iter().all(|change| {
            change
                .location
                .as_ref()
                .is_some_and(|location| location.path.as_os_str().is_empty())
        }));
        assert!(
            changes
                .iter()
                .all(|change| change.source_path() == Some(std::path::Path::new("api.php")))
        );
    }

    #[test]
    fn promoted_parameter_defaults_are_not_property_defaults() {
        let (_old_root, old) = snapshot(
            r#"<?php
namespace Test;
class Api { public function __construct(public int $id = 1) {} }
"#,
        );
        let (_new_root, new) = snapshot(
            r#"<?php
namespace Test;
class Api { public function __construct(public int $id = 2) {} }
"#,
        );

        let rendered = compare(&old, &new)
            .into_iter()
            .map(|change| change.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            ["[BC] CHANGED: Default parameter value for parameter $id of Test\\Api#__construct() changed from 1 to 2"]
        );
    }

    #[test]
    fn defaults_before_required_parameters_are_not_available_through_reflection() {
        let (_old_root, old) = snapshot(
            r#"<?php
namespace Test;
class Api { public function run($optional = 1, $required) {} }
"#,
        );
        let (_new_root, new) = snapshot(
            r#"<?php
namespace Test;
class Api { public function run($optional = 2, $required) {} }
"#,
        );

        assert!(compare(&old, &new).is_empty());
    }

    #[test]
    fn inaccessible_members_are_reported_as_removed_before_member_changes() {
        let (_old_root, old) = snapshot(
            r#"<?php
namespace Test;
class Api {
    public const VALUE = 1;
    protected string $value = 'x';
    public function run(): void {}
}
"#,
        );
        let (_new_root, new) = snapshot(
            r#"<?php
namespace Test;
class Api {
    private const VALUE = 1;
    /** @internal */
    protected string $value = 'x';
    private function run(): void {}
}
"#,
        );

        let rendered = compare(&old, &new)
            .into_iter()
            .map(|change| change.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            [
                "[BC] REMOVED: Constant Test\\Api::VALUE was removed",
                "[BC] REMOVED: Property Test\\Api#$value was removed",
                "[BC] REMOVED: Method Test\\Api#run() was removed",
                "[BC] CHANGED: Constant Test\\Api::VALUE visibility reduced from public to private",
                "[BC] CHANGED: Property Test\\Api#$value was marked \"@internal\"",
                "[BC] CHANGED: Method run() of class Test\\Api visibility reduced from public to private",
            ]
        );
    }

    #[test]
    fn enum_case_changes_keep_the_four_upstream_groups() {
        let (_old_root, old) = snapshot(
            r#"<?php
namespace Test;
enum State {
    case Removed;
    case Internalized;
    /** @internal */
    case Publicized;
    /** @internal */
    case InternalRemoved;
}
"#,
        );
        let (_new_root, new) = snapshot(
            r#"<?php
namespace Test;
enum State {
    case Added;
    /** @internal */
    case InternalAdded;
    /** @internal */
    case Internalized;
    case Publicized;
}
"#,
        );

        let rendered = compare(&old, &new)
            .into_iter()
            .map(|change| change.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            [
                "[BC] REMOVED: Case Test\\State::Removed was removed",
                "[BC] ADDED: Case Test\\State::Added was added",
                "[BC] CHANGED: Case Test\\State::Internalized was marked \"@internal\"",
                "[BC] CHANGED: Case Test\\State::Publicized had \"@internal\" removed",
            ]
        );
    }
}
