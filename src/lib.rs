//! Programmatic API for comparing two revisions of a PHP library.
//!
//! [`check_repository`] runs the same isolated Git, dependency-installation,
//! parsing, comparison, and baseline pipeline as the `legato` command. Finding
//! classification is provided by the typed rules re-exported from
//! [`legato_rules`].
//!
//! # Example
//!
//! ```no_run
//! use std::path::PathBuf;
//!
//! use legato::{CheckOptions, check_repository};
//!
//! let report = check_repository(CheckOptions {
//!     repository: PathBuf::from("."),
//!     from_revision: Some("v1.0.0".to_owned()),
//!     to_revision: "HEAD".to_owned(),
//!     install_development_dependencies: false,
//! })?;
//!
//! for change in report.changes {
//!     println!("{}: {}", change.identifier(), change.description);
//! }
//! # Ok::<(), legato::CheckError>(())
//! ```

#![deny(missing_docs)]

use std::path::PathBuf;

pub use change::{Change, SourceLocation};
pub use error::CheckError;
pub use legato_rules::{CompatibilityImpact, ModificationType, Rule, RuleCategory, RuleMetadata};

/// Backwards-compatible name for callers that only used stable rule identifiers.
pub type ChangeIdentifier = Rule;

mod change;
mod compare;
mod composer;
mod config;
mod error;
mod output;
mod repository;
mod snapshot;
mod value;

/// Command-line entry point and shell-completion support.
pub mod cli;

/// Inputs for one repository comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOptions {
    /// Path inside the Git working tree to compare.
    ///
    /// Legato resolves this to the repository root before loading an optional
    /// `legato.toml` and creating temporary checkouts.
    pub repository: PathBuf,
    /// Base Git reference, or [`None`] to select the highest stable version tag.
    pub from_revision: Option<String>,
    /// Target Git reference compared against `from_revision`.
    pub to_revision: String,
    /// Whether Composer packages from `require-dev` should provide analysis context.
    pub install_development_dependencies: bool,
}

/// Result of a completed repository comparison after baseline filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    /// Resolved base commit ID.
    pub from_revision: String,
    /// Resolved target commit ID.
    pub to_revision: String,
    /// Findings that were not suppressed by the configured baseline.
    pub changes: Vec<Change>,
}

/// Compare the configured PHP API between two Git revisions.
///
/// A repository may optionally contain `legato.toml`; otherwise the default
/// PHP 8.5.9 platform is used with all extensions assumed. All dependency
/// installation and parsing happens in temporary checkouts; the supplied
/// working tree is not modified.
///
/// # Errors
///
/// Returns [`CheckError`] when repository validation, revision resolution,
/// configuration, dependency installation, parsing, extraction, or I/O fails.
pub fn check_repository(options: CheckOptions) -> Result<CheckReport, CheckError> {
    repository::check(options, repository::Observer::default())
}
