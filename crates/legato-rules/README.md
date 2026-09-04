# legato-rules

`legato-rules` is the typed compatibility-rule registry used by
[Legato](https://github.com/shyim/legato). It gives each finding one stable
identity and defines that rule's category, modification type, and compatibility
impact in one place.

```rust
use legato_rules::{ModificationType, Rule, RuleCategory};

let rule = Rule::MethodParameterTypeChanged;

assert_eq!(rule.identifier(), "method.parameter-type-changed");
assert_eq!(rule.category(), RuleCategory::Method);
assert_eq!(rule.modification_type(), ModificationType::Changed);
assert!(rule.is_breaking());
```

`Rule::ALL` exposes the complete registry in canonical order. Stable string
identifiers implement `Display` and `FromStr`; enable the `serde` feature to
serialize and deserialize them as those same strings.

The crate deliberately does not contain detector logic or rendered finding
descriptions. Legato's comparator emits a `Rule`, and consumers derive all
classification metadata from it. This prevents contradictory findings such as
a `method.removed` rule labeled as an added or informational change.
