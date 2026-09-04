# Compatibility contract

This port follows `roave/backward-compatibility-check` 8.22.x at commit
`4d1572f31e71415bce6605634a88e3d05a64dae5`.

This document records the boundary of the port. [`RULES.md`](RULES.md) is the
normative detector catalog, while
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) describes Legato's implementation.

## Preserved behavior

- Only named classes, interfaces, traits, and enums from the old root package's
  Composer `autoload` paths define the API under comparison.
- Root `autoload-dev`, top-level functions, and top-level constants are not part
  of the comparison.
- Anonymous and old `@internal` class-like declarations are excluded.
- Dependencies and PHP built-ins provide type and inheritance context, but do
  not themselves define root-package API.
- Detector ordering, change categories, descriptions, and the historical
  spelling quirks in those descriptions are compatibility-sensitive.
- `skipped` findings are backwards-incompatible and contribute to exit code `3`.
- Full-description ignored regexes, console, Markdown, GitHub Actions, JSON, and
  JUnit output are supported.
- Source locations are 1-based and paths are relative to the checked-out target
  revision whenever a target declaration exists.
- Finding descriptions and Markdown output retain the upstream wording.

## Intentional differences

- The executable is named `legato` and exposes checker flags plus
  `--help`/`--version`; Symfony Console's generic flags and named subcommand are
  not reproduced.
- The unconditional political startup banner from the PHP executable is not
  emitted.
- The default console formatter is Legato-native: it groups findings by file
  and adds revisions, source positions, severities, rule identifiers, totals,
  and baseline guidance. Finding descriptions remain compatibility-stable.
- Structured formatter output is kept valid: embedded Riff progress and success
  output are suppressed and dependency diagnostics stay on stderr.
- Findings have stable machine-readable identifiers. JSON exposes them as
  `identifier` alongside the affected-file `sourcePath`, GitHub Actions
  annotations use them as `title`, and JUnit uses them as the testcase
  `classname`; these fields intentionally extend the upstream structured output
  metadata.
- Relative source paths do not carry the upstream accidental leading slash.
- Temporary checkouts are cleaned up after successful and failed comparisons.
- Parsing semantics are fixed to PHP 8.5.
- The upstream XML configuration is replaced by repository-root
  `legato.toml`, which is optional. Dependency resolution defaults to PHP
  8.5.9, all extensions and native platform capabilities are assumed, and the
  file can override the exact PHP version, exclude root-package paths, or
  suppress findings by identifier and/or source-path glob. Legacy XML files
  are rejected with a migration error.
- PHP-internal source paths are empty as upstream's are, while their line and
  column positions come from Mago's embedded PHP 8.5 stubs rather than the
  host runtime's generated BetterReflection stubs.
- Scalar, arithmetic, concatenation, and array declaration values are evaluated
  directly. Other constant expressions are compared by their name-resolved Mago
  fingerprint; a changed unresolved expression produces a generic `skipped`
  finding instead of BetterReflection's exception-specific text.

## Rule inventory

The comparison includes class-like removal and kind changes, new final/abstract
or internal declarations, ancestor removal, enum case changes, interface method
addition, accessible constant/property/method removal, constant value and
visibility changes, property visibility/scope/type/default/internal changes,
method finality/concreteness/visibility/scope changes, and function-signature
checks for reference passing, defaults, required arity, parameter names/types,
return types, and the `@no-named-arguments` annotation.

Every emitted rule has a stable identifier, which may be used by a structured
TOML baseline without depending on human-readable text. The complete identifier
catalog, exact trigger conditions, and before/after examples are documented in
[RULES.md](RULES.md).

Native declaration types use the upstream detector's intentionally simple
covariance and contravariance rules. PHPDoc types are not compared.
