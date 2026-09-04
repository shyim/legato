# Contributing

Contributions should preserve Legato's deterministic comparison and output
contracts. Detector behavior is compatibility-sensitive, so tests and
documentation are part of every functional change.

## Prerequisites

- Git
- Rust 1.98.0, pinned by `rust-toolchain.toml`

PHP and Composer are not required. Dependency installation tests use the
embedded Riff API and may need network access on their first run.

If Rust is managed through mise, prefix Cargo commands with
`mise exec rust@1.98.0 --`.

## Repository layout

- `src/` contains the CLI, orchestration, parser model, detectors, and output.
- `crates/legato-rules/` contains the public typed rule registry.
- `tests/cli_e2e.rs` exercises real Git revisions through the CLI.
- `RULES.md` is the rule-by-rule behavioral contract.
- `PARITY.md` records preserved upstream behavior and intentional differences.
- `docs/` contains configuration, output, and architecture guides.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the end-to-end pipeline
and module ownership.

## Local validation

Run the same checks as CI from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
RUSTDOCFLAGS="-D warnings" cargo test --doc --workspace --locked
cargo test --workspace --all-targets --locked
cargo build --release --workspace --locked
git diff --check
```

Use `cargo fmt --all` to apply formatting. Do not commit `target/`, coverage
data, release archives, or temporary dependency installations.

## Changing a compatibility rule

A rule change must update all layers of the contract:

1. Add or modify its entry in the `rules!` registry in
   `crates/legato-rules/src/lib.rs`.
2. Emit the typed `Rule` from the detector in `src/compare.rs`. Modification
   type and compatibility impact must not be duplicated in detector code.
3. Add a focused detector assertion when wording or ordering matters, and
   extend `every_registered_rule_has_a_behavioral_fixture` so the rule is
   reached through real old/new PHP snapshots.
4. Document the exact condition and a minimal before/after example in
   `RULES.md`.

Completeness tests intentionally fail when the registry, metadata fixture,
Serde identifiers, documentation table, or behavioral fixture drift apart.

## Changing output

Keep machine-readable data on stdout and human progress/console output on
stderr. Update formatter goldens and [`docs/OUTPUT.md`](docs/OUTPUT.md) when a
schema, field meaning, stream, or exit-code behavior changes. Stable rule
identifiers are preferred over descriptions for automation.

## Changing dependency preparation

Composer metadata remains the input format, while Riff is the only dependency
installer. Do not add host PHP or Composer probing. Preserve these safeguards:

- one exact PHP package version, from the built-in default or configuration,
  for both revisions;
- plugins, scripts, audits, and autoloader generation disabled;
- extension and native platform requirements assumed present;
- paths constrained to the temporary checkout; and
- lockless updates isolated from the caller's working tree.

Exercise both locked and lockless projects in the embedded-Riff tests after an
integration change. Keep Git dependencies pinned to an exact revision and
commit the matching `Cargo.lock` update.

## Pull requests

Keep commits focused and include the motivation, user-visible effects, and
validation performed. Update public documentation in the same change as the
behavior it describes. Do not silently broaden the supported API boundary;
record intentional parity differences in `PARITY.md`.
