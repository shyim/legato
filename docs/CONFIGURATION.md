# Configuration

`legato.toml` is optional at the root of the repository being checked. When it
is present, the file is loaded before temporary checkouts are created, so one
configuration applies to both revisions.

Without the file, Legato uses PHP 8.5.9 for dependency resolution, assumes all
PHP extensions and native platform capabilities are available, excludes no
project paths, and applies no baseline. An empty file has the same behavior.

## Complete example

```toml
[platform]
php = "8.5.9"
extensions = "all"

[paths]
exclude = [
    "tests/**",
    "src/Internal/**",
    "src/Generated/",
]

[baseline]
ignored_regex = [
    '#^\[BC\] CHANGED: The parameter \$id of Acme\\Api\#find\(\)#',
]

[[baseline.ignore]]
identifier = "method.parameter-type-changed"

[[baseline.ignore]]
identifier = "class.removed"
path = "src/Legacy/**"

[[baseline.ignore]]
path = "src/Generated/Api.php"
```

Unknown sections and keys are rejected rather than silently ignored.

## Platform

| Key | Required | Meaning |
| --- | --- | --- |
| `platform.php` | no; defaults to `"8.5.9"` | Exact stable semantic version used by Riff when resolving both revisions |
| `platform.extensions` | no; defaults to `"all"` | Must currently be `"all"` |

The entire `[platform]` section may be omitted. An explicitly supplied
`platform.php` must include major, minor, and patch components and cannot have
prerelease or build metadata. It controls dependency constraint resolution; it
does not select a local PHP executable. Legato never probes or invokes PHP.

`extensions = "all"` means extension, native-library, and derived PHP platform
requirements are treated as satisfied. The configured PHP package constraint
is still enforced. Source parsing and the embedded built-in API currently use
PHP 8.5; this intentional distinction is recorded in
[`PARITY.md`](../PARITY.md).

## Project path exclusions

`paths.exclude` is a list of repository-relative glob patterns. Matching root
package files are excluded before snapshot construction. Dependency sources are
not excluded because they remain necessary type and inheritance context.

- `/`, drive-letter, and parent-directory paths are rejected.
- Backslashes are normalized to `/`.
- A trailing `/` is treated as `/**`.
- A pattern ending in `/**` matches both the directory and its descendants.
- Glob separators are literal, so `*` does not cross `/`; use `**` for nested
  paths.

Exclusions change the analyzed API. Use a baseline instead when a declaration
should remain analyzed but one known finding is intentional.

## Structured baseline entries

Each `[[baseline.ignore]]` entry accepts `identifier`, `path`, or both:

| Selector | Matching input |
| --- | --- |
| `identifier` | Stable rule identifier from [`RULES.md`](../RULES.md) |
| `path` | Repository-relative affected source path |

Entries are alternatives. When one entry supplies both selectors, both must
match the same finding. Empty entries and unknown identifiers are errors.

For a changed declaration, the source path normally points into the target
revision. A removed declaration falls back to its old source path. Findings
without a usable path cannot match a path-only baseline entry.

Structured entries are preferred because they do not depend on human-readable
wording.

## Regex baselines

`baseline.ignored_regex` accepts `#`-delimited PCRE2 expressions using the same
shape as the old PHP configuration. Expressions match the complete rendered
finding:

```text
[BC] CHANGED: The parameter $id of Acme\Api#find() changed from string to int
```

The `[BC]` prefix and modification label are part of the input. Common PHP
modifiers are supported, invalid patterns fail configuration loading, and an
operational error cannot be suppressed by a baseline.

Regex baselines exist for migration and wording-specific exceptions. Prefer a
structured identifier/path entry for new suppressions.

## Development dependencies

Packages from Composer's `require-dev` section are not installed by default.
Enable them when they provide inheritance or type context needed by the public
API:

```sh
legato --install-development-dependencies
```

This is a command-line option rather than a configuration key.

## Migration

- Rename the early development filename `php-bc-check.toml` to `legato.toml`.
- The upstream `.roave-backward-compatibility-check.xml` format is not loaded.
  Move each `<ignored-regex>` value into `baseline.ignored_regex`, then remove
  the XML file.

Legato reports either obsolete filename with a migration error so stale
configuration cannot be overlooked.
