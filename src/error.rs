use std::path::PathBuf;

/// Failure produced while preparing or comparing a repository.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CheckError {
    /// The requested path is not inside a usable Git working tree.
    #[error("Directory \"{}\" is not a GIT repository.", .0.display())]
    InvalidRepository(PathBuf),

    /// Git could not resolve or prepare the requested revisions.
    #[error("Git operation failed: {0}")]
    Git(String),

    /// Riff could not prepare Composer dependency context.
    #[error("Dependency installation failed: {0}")]
    Composer(String),

    /// `legato.toml` is invalid, or an obsolete configuration file is present.
    #[error("Invalid configuration: {0}")]
    Configuration(String),

    /// Mago could not parse one or more PHP sources.
    #[error("Unable to parse PHP sources: {0}")]
    Parse(String),

    /// A parsed source could not be converted into the comparison model.
    #[error("Unable to extract PHP API: {0}")]
    Extraction(String),

    /// Filesystem or process I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Composer metadata could not be decoded as JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
