# Compatibility rules

Legato compares the API exposed by the old revision with the API exposed by the
target revision. This document describes the current detector behavior, not a
general definition of semantic versioning.

Every finding listed below is currently backwards-incompatible. This includes
`ADDED` findings and `SKIPPED` findings where Legato cannot safely compare two
values. Any emitted finding therefore contributes to exit code `3`.

The canonical typed registry lives in
[`crates/legato-rules`](https://github.com/shyim/legato/tree/main/crates/legato-rules).
It owns each identifier's API category, modification type, and compatibility
impact. Tests require this catalog to list every registered identifier in
canonical order and exercise every rule through the real comparison pipeline.

Examples show the smallest relevant declaration fragment as `before` ->
`after`. Namespaces and unrelated method bodies are omitted. A single source
change can activate more than one rule; for example, an incompatible method
type change can produce both a variance finding and an exact-signature finding.

## API boundary

The comparison starts from named classes, interfaces, traits, and enums found
in the old root package's Composer `autoload` paths.

- Anonymous declarations and old declarations marked `@internal` are excluded.
- Root `autoload-dev`, top-level functions, and top-level constants are not
  compared.
- Dependencies and PHP built-ins provide inheritance and type context, but
  their declarations do not independently define the root package's API.
- Inherited and trait-provided members are compared as part of the effective API
  of a project declaration.
- For an open class, public and protected non-internal properties and methods
  are generally considered. For a final class, property and method checks are
  generally limited to public non-internal members. Interfaces and traits use
  the upstream checker's specialized member-selection rules.
- Native declaration types are compared. PHPDoc types are not.
- `@internal` and `@no-named-arguments` are the PHPDoc annotations with explicit
  compatibility rules.

## Class rules

The `class.*` prefix is historical. `class.removed` applies to every supported
class-like declaration, and `class.became-internal` is also used for interfaces
and traits.

| Identifier | Fires when | Minimal example (`before` -> `after`) |
| --- | --- | --- |
| `class.removed` | A named old class, interface, trait, or enum is absent from the target API. | `class Api {}` -> declaration deleted |
| `class.became-abstract` | A concrete class becomes abstract. | `class Api {}` -> `abstract class Api {}` |
| `class.became-interface` | A class becomes an interface. | `class Api {}` -> `interface Api {}` |
| `class.became-trait` | A class becomes a trait. | `class Api {}` -> `trait Api {}` |
| `class.became-final` | A non-final class becomes final. | `class Api {}` -> `final class Api {}` |
| `class.became-internal` | A previously public class-like declaration gains `@internal`. | `class Api {}` -> `/** @internal */ class Api {}` |
| `class.ancestor-removed` | A class loses an effective parent class or implemented interface. | `class Api extends Base implements Contract {}` -> `class Api {}` |
| `class.became-enum` | A class becomes an enum. | `class Status {}` -> `enum Status {}` |

## Interface rules

| Identifier | Fires when | Minimal example (`before` -> `after`) |
| --- | --- | --- |
| `interface.became-class` | An interface becomes a class or enum. | `interface Api {}` -> `class Api {}` |
| `interface.became-trait` | An interface becomes a trait. | `interface Api {}` -> `trait Api {}` |
| `interface.ancestor-removed` | An interface loses an effective parent interface. | `interface Api extends Base {}` -> `interface Api {}` |
| `interface.method-added` | The effective method set of an existing interface gains a method. Implementors would have a new requirement. | `interface Api {}` -> `interface Api { public function run(): void; }` |

## Trait rules

| Identifier | Fires when | Minimal example (`before` -> `after`) |
| --- | --- | --- |
| `trait.became-interface` | A trait becomes an interface. | `trait Shared {}` -> `interface Shared {}` |
| `trait.became-class` | A trait becomes a class or enum. | `trait Shared {}` -> `class Shared {}` |

Trait member removals and mutations use the constant, property, and method rules
below. The human-readable `trait.became-interface` description retains an
upstream wording typo for output compatibility.

## Enum rules

Adding a public enum case is intentionally considered breaking because callers
may perform exhaustive matches over the old case set. Internal enum cases are
ignored until their public/internal status changes.

| Identifier | Fires when | Minimal example (`before` -> `after`) |
| --- | --- | --- |
| `enum.kind-changed` | An enum becomes any non-enum class-like kind. | `enum Status {}` -> `class Status {}` |
| `enum.case-removed` | A non-internal enum case is removed. | `enum Status { case Ready; }` -> `enum Status {}` |
| `enum.case-added` | A non-internal enum case is added. | `enum Status {}` -> `enum Status { case Ready; }` |
| `enum.case-became-internal` | An existing public case gains `@internal`. | `case Ready;` -> `/** @internal */ case Ready;` |
| `enum.case-internal-removed` | An existing internal case loses `@internal` and becomes public. | `/** @internal */ case Ready;` -> `case Ready;` |

## Constant rules

Constant rules include inherited constants. A constant that becomes private is
treated as removed from the accessible API. Value comparisons use PHP-style
strict scalar, array, arithmetic, and concatenation evaluation where supported.

| Identifier | Fires when | Minimal example (`before` -> `after`) |
| --- | --- | --- |
| `constant.removed` | An old non-private constant is missing or becomes private. | `public const MODE = 1;` -> declaration deleted |
| `constant.visibility-reduced` | A selected class or enum constant becomes less visible. | `public const MODE = 1;` -> `protected const MODE = 1;` |
| `constant.value-changed` | Two supported constant values are not strictly equal. | `public const MODE = 1;` -> `public const MODE = 2;` |
| `constant.value-comparison-unsupported` | Unequal constant expressions cannot be safely evaluated and compared. The finding is `SKIPPED` but breaking. | `public const MODE = OLD_MODE;` -> `public const MODE = NEW_MODE;` |

## Property rules

Property types are effectively invariant: a change is accepted only when the
new type is both covariant and contravariant with the old type. Default values
are compared strictly using the same value model as constants.

| Identifier | Fires when | Minimal example (`before` -> `after`) |
| --- | --- | --- |
| `property.removed` | An accessible property is missing or no longer accessible. | `public string $name;` -> declaration deleted |
| `property.became-internal` | A selected property gains `@internal`. | `public string $name;` -> `/** @internal */ public string $name;` |
| `property.type-changed` | A selected native property type is not equivalent under both variance checks. | `public string $name;` -> `public int $name;` |
| `property.default-value-changed` | Two supported default values are not strictly equal. | `public int $page = 1;` -> `public int $page = 2;` |
| `property.default-value-comparison-unsupported` | Unequal default expressions cannot be safely evaluated and compared. The finding is `SKIPPED` but breaking. | `public int $mode = OLD_MODE;` -> `public int $mode = NEW_MODE;` |
| `property.visibility-reduced` | A selected property becomes less visible. | `public string $name;` -> `protected string $name;` |
| `property.scope-changed` | A selected property changes between instance and static scope. | `public string $name;` -> `public static string $name;` |

## Method rules

Method rules operate on the effective method set, including inherited and
trait-provided methods. Exact type and parameter-name checks are not run in the
special final-class mode, but variance, reference, arity, and default-value
checks still apply.

Adding a parameter to an overridable method is reported even when the new
parameter is optional. Optional parameters added to constructors are exempt,
as are additions to methods declared by final classes or old private methods.

| Identifier | Fires when | Minimal example (`before` -> `after`) |
| --- | --- | --- |
| `method.removed` | An accessible method is missing or no longer accessible. | `public function run(): void {}` -> declaration deleted |
| `method.became-final` | A selected non-final method becomes final. | `public function run(): void {}` -> `final public function run(): void {}` |
| `method.became-abstract` | A selected concrete method becomes abstract. | `public function run(): void {}` -> `abstract public function run(): void;` |
| `method.scope-changed` | A selected method changes between instance and static scope. | `public function run(): void {}` -> `public static function run(): void {}` |
| `method.visibility-reduced` | A selected non-interface method becomes less visible. | `public function run(): void {}` -> `protected function run(): void {}` |
| `method.parameter-added` | An overridable method gains a parameter name that did not exist in the old signature, subject to the exemptions above. | `function run(string $id)` -> `function run(string $id, bool $force = false)` |
| `method.became-internal` | A selected method gains `@internal`. | `public function run(): void {}` -> `/** @internal */ public function run(): void {}` |
| `method.parameter-reference-changed` | A parameter changes between by-value and by-reference. | `function run(string $id)` -> `function run(string &$id)` |
| `method.return-reference-changed` | The return value changes between by-value and by-reference. | `function value(): string` -> `function &value(): string` |
| `method.required-parameter-count-increased` | The number of required positional arguments increases. | `function run(string $id = '')` -> `function run(string $id)` |
| `method.parameter-default-value-changed` | Two available, supported parameter defaults are not strictly equal. | `function run(int $page = 1)` -> `function run(int $page = 2)` |
| `method.parameter-default-value-comparison-unsupported` | Unequal default expressions cannot be safely evaluated and compared. The finding is `SKIPPED` but breaking. | `function run(int $mode = OLD_MODE)` -> `function run(int $mode = NEW_MODE)` |
| `method.return-type-non-covariant` | The new return type is not covariant with the old return type. | `function make(): Dog` -> `function make(): Animal` |
| `method.return-type-changed` | The rendered return type changes where exact-signature checks apply, even if the change is covariant. | `function make(): Animal` -> `function make(): Dog` |
| `method.parameter-type-non-contravariant` | A new parameter type is not contravariant with the old parameter type. | `function accept(Animal $value)` -> `function accept(Dog $value)` |
| `method.parameter-type-changed` | A rendered parameter type changes where exact-signature checks apply, even if the change is contravariant. | `function accept(Dog $value)` -> `function accept(Animal $value)` |
| `method.no-named-arguments-removed` | `@no-named-arguments` is removed from the method or its declaring class. | `/** @no-named-arguments */ function run($id)` -> `function run($id)` |
| `method.no-named-arguments-added` | `@no-named-arguments` is added to the method or its declaring class. | `function run($id)` -> `/** @no-named-arguments */ function run($id)` |
| `method.parameter-name-changed` | A parameter name changes while named arguments remain supported. | `function run($id)` -> `function run($userId)` |

`Dog` is assumed to extend or implement `Animal` in the variance examples.
An incompatible type replacement can emit both the variance and exact-change
identifiers. This is expected; the rules represent separate upstream checks.

## Changes not currently detected

Legato does not currently report every source change that might matter to an
application. In particular, the detector does not cover:

- additions of classes, ordinary class methods, properties, or constants;
- top-level functions or top-level constants;
- declarations reachable only through root `autoload-dev`;
- PHPDoc type, template, exception-contract, or deprecation changes;
- method body behavior, side effects, performance, or runtime data-format
  changes;
- readonly changes or general PHP attribute changes;
- visibility increases, removal of `final`, or other changes that only widen
  the modeled API; and
- old private/internal members outside the specialized interface and trait
  behavior inherited from the upstream checker.

Configuration can narrow the analyzed source paths or suppress findings, but it
does not change these rule semantics. See the
[configuration guide](docs/CONFIGURATION.md) for exclusions and baselines, and
[PARITY.md](PARITY.md) for the upstream compatibility boundary and intentional
implementation differences.
