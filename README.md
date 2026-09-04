# Legato

[![CI](https://github.com/shyim/legato/actions/workflows/ci.yml/badge.svg)](https://github.com/shyim/legato/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Legato is a fast, standalone backwards-compatibility checker for PHP libraries,
written in Rust. It compares the Composer-defined public API of two Git
revisions and reports changes that can break downstream users.

Legato is a Rust port of
[`roave/backward-compatibility-check`](https://github.com/Roave/BackwardCompatibilityCheck).
It uses [Mago](https://github.com/carthage-software/mago) to parse PHP and embeds
[Riff](https://github.com/shyim/riff) to prepare each revision's dependency
context. PHP and Composer are not required at runtime.

Legato is under active development. The initial release targets Linux x86-64
and follows the upstream 8.22.x rule set. See [Compatibility](PARITY.md) for the
precise behavior and intentional differences.

## Why Legato?

- A native binary with no PHP, Composer, or project bootstrap requirement.
- Composer-aware API discovery from the root package's `autoload` paths.
- Dependency and PHP built-in context for inheritance and type resolution.
- Console, Markdown, GitHub Actions, JSON, and JUnit output for local and CI
  workflows.
- Stable finding identifiers plus path and regex baselines for intentional
  breaks.
- Isolated temporary checkouts and dependency installs for both revisions.
- Compatibility with the established Roave detector ordering and messages.

## Documentation

| Guide | Covers |
| --- | --- |
| [Configuration](docs/CONFIGURATION.md) | Platform selection, exclusions, baselines, and migration |
| [Compatibility rules](RULES.md) | Stable identifiers, exact conditions, and examples |
| [Output formats](docs/OUTPUT.md) | Streams, schemas, colors, and exit codes |
| [Architecture](docs/ARCHITECTURE.md) | Pipeline, invariants, concurrency, and module ownership |
| [Upstream parity](PARITY.md) | Preserved behavior and intentional differences |
| [Contributing](CONTRIBUTING.md) | Development workflow and validation contract |

## Install a release binary

Tagged releases publish a prebuilt Linux x86-64 binary on
[GitHub Releases](https://github.com/shyim/legato/releases). Download the
archive, verify it with the release's `SHA256SUMS`, and place `legato` on your
`PATH`.

## Install from source

Building Legato requires Git and Rust 1.98.0. It does not require PHP or
Composer.

```sh
git clone https://github.com/shyim/legato.git
cd legato
cargo install --path . --locked
legato --version
```

## Quick start

Run Legato from the PHP library's Git repository:

```sh
legato
```

No configuration file is required. By default, Legato resolves dependencies
for PHP 8.5.9 and assumes all PHP extensions and native platform capabilities
are available.

Add `legato.toml` when the project needs a different exact PHP version, path
exclusions, or baseline entries:

```toml
[platform]
php = "8.5.9"
extensions = "all"
```

With no `--from` option, Legato compares the highest stable semantic-version
tag with `HEAD`. Tags may start with `v` or `release-`; prerelease tags are
ignored. Select both revisions explicitly when needed:

```sh
legato --from=1.4.0 --to=HEAD
```

Development dependencies are excluded from the temporary installs by default.
Include them when they are needed for API resolution:

```sh
legato --install-development-dependencies
```

Run `legato --help` for the complete option list.

Interactive console output uses color when stderr is a terminal. Use
`--color=always` to preserve color through a compatible pipe or
`--color=never` to disable it. Auto mode also honors `NO_COLOR`, `CLICOLOR`,
and `CLICOLOR_FORCE`.

## Shell completions

Legato generates dynamic completion scripts for Bash, Zsh, Fish, Nushell, and
PowerShell from the same `usage-rs` declaration that parses its command line.
For the current shell session, source the generated script:

```sh
# Bash
source <(legato completion bash)

# Zsh
source <(legato completion zsh)

# Fish
legato completion fish | source
```

Use `legato completion nu` or `legato completion powershell` to generate scripts
for Nushell or PowerShell. Store the output in the shell's completion directory
to enable it persistently.

## Configuration

`legato.toml` is optional. Without it, Legato uses PHP 8.5.9 for dependency
resolution and assumes all extension and native platform requirements are
present. A `[platform]` section can override the exact PHP version; its fields
inherit the defaults when omitted. Legato never probes or invokes PHP.

Optional configuration can exclude root-package paths before analysis or
suppress known findings by stable identifier, affected-path glob, or
full-description regex. The complete keys, matching rules, examples, and legacy
migration steps are in the [configuration guide](docs/CONFIGURATION.md).

See [RULES.md](RULES.md) for the identifier catalog and exact detector contract.
Every current rule, including Added and Skipped findings, is
backwards-incompatible and contributes to exit code `3`.

## Output and exit codes

The default console report shows the compared revisions, groups findings by
affected file, and includes source positions, severity, change category, and
the stable rule identifier used by `[[baseline.ignore]]`:

```text
Legato compatibility report
  0123456789ab → fedcba987654
  1 finding · 1 affected file

src/Api.php
  7:28  BREAKING  CHANGED  The parameter $value of Acme\Api#change() changed type
                            rule: method.parameter-type-changed

Summary
  1 breaking change · 0 informational findings
  1 changed
```

Console output is written to stderr and follows the selected `--color` mode.
Machine-readable and document formats are written to stdout:

```sh
legato --format=markdown > changes.md
legato --format=json > changes.json
legato --format=github-actions
legato --format=junit > junit.xml
```

`--format` may be repeated. JSON exposes each finding's stable `identifier` and
affected `sourcePath`; GitHub Actions uses the identifier as the annotation
title, and JUnit uses it as the testcase classname. Progress messages and the
human-readable report are written only when `console` is among the
selected formats, so `legato --format json` emits only the JSON document.

See [Output formats](docs/OUTPUT.md) for field semantics and integration
examples.

Legato exits with:

- `0` when no backwards-incompatible changes are found.
- `3` when one or more backwards-incompatible changes are found.
- `1` for invalid input or an operational failure.

## How it works

Legato resolves the selected Git revisions into isolated temporary checkouts.
Riff installs each revision's locked dependencies—or creates a temporary lock
for lockless libraries—without plugins, scripts, audits, or autoloader
generation. Mago then builds an API model from root-package sources, using
dependency sources and embedded PHP 8.5 stubs only as analysis context.

After comparison, both checkouts and their `vendor` directories are removed.
The original working tree is never modified.

The
[`legato-rules`](https://github.com/shyim/legato/tree/main/crates/legato-rules)
workspace crate is the canonical typed registry for finding identifiers,
categories, modification types, and compatibility impact. Detectors emit a
rule plus a description and location; all output formats and baseline matching
derive classification metadata from that rule.

The complete pipeline and its isolation, platform, API-boundary, and stream
invariants are documented in [Architecture](docs/ARCHITECTURE.md).

## Development

Rust 1.98.0 is pinned in `rust-toolchain.toml`.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
RUSTDOCFLAGS="-D warnings" cargo test --doc --workspace --locked
cargo test --workspace --all-targets --locked
cargo build --release --workspace --locked
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the repository layout, rule-change
workflow, invariants, and full validation expectations.

## License

Legato is available under the [MIT License](LICENSE).
