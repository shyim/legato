//! Command-line execution and shell-completion support.

use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anstream::{AutoStream, ColorChoice as StreamColorChoice};
use anstyle::{AnsiColor, Color, Style};

use crate::output::{self, OutputFormat};
use crate::repository::{self, Observer};
use crate::{CheckError, CheckOptions};

const OPERATIONAL_ERROR_CODE: u8 = 1;
const CHANGES_DETECTED_CODE: u8 = 3;
const MUTED_STYLE: Style = Style::new().dimmed();
const DANGER_STYLE: Style = Color::Ansi(AnsiColor::Red).on_default().bold();

/// Verify that a PHP library remains backward compatible between two Git revisions.
///
/// With no `--from` value, the highest stable semantic-version tag is used. The command
/// creates two isolated checkouts, installs their Composer dependencies without plugins or
/// scripts, and compares the APIs declared by the root package's `autoload` section.
#[derive(Debug, PartialEq, Eq, usage::Cli)]
#[usage(
    bin = "legato",
    version = env!("CARGO_PKG_VERSION"),
    unknown_flags = "error",
    completion,
    usage = "Usage: legato [FLAGS]\n       legato completion <SHELL>"
)]
struct Arguments {
    /// Git reference for the stable base version. Omit the option or its value to detect it.
    #[usage(long, value_name = "FROM")]
    from: Option<Option<String>>,

    /// Git reference for the new version checked against `--from`.
    #[usage(long, value_name = "TO", default = "HEAD")]
    to: String,

    /// Output format: console, markdown, github-actions, json, or junit.
    #[usage(
        long,
        value_name = "FORMAT",
        var,
        default = "console",
        default_missing = "console",
        choices("console", "markdown", "github-actions", "json", "junit")
    )]
    format: Vec<OutputFormat>,

    /// Colorize human-readable output: auto, always, or never.
    #[usage(long, value_name = "WHEN", value_enum, default = "auto", default_missing = "always")]
    color: ColorMode,

    /// Also install packages from Composer's `require-dev` section.
    #[usage(long)]
    install_development_dependencies: bool,

    #[usage(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, usage::ValueEnum)]
enum ColorMode {
    /// Color only when stderr supports it.
    Auto,
    /// Always emit colored output.
    Always,
    /// Never emit colored output.
    Never,
}

impl From<ColorMode> for StreamColorChoice {
    fn from(mode: ColorMode) -> Self {
        match mode {
            ColorMode::Auto => Self::Auto,
            ColorMode::Always => Self::Always,
            ColorMode::Never => Self::Never,
        }
    }
}

#[derive(Debug, PartialEq, Eq, usage::Subcommands)]
enum Command {
    /// Generate a completion script for a shell.
    Completion(CompletionArguments),
}

#[derive(Debug, PartialEq, Eq, usage::Args)]
struct CompletionArguments {
    /// Shell whose completion script should be generated.
    #[usage(value_enum)]
    shell: CompletionShell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, usage::ValueEnum)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    #[usage(name = "nu", visible_alias = "nushell")]
    Nu,
    #[usage(name = "powershell", visible_alias = "pwsh")]
    PowerShell,
}

impl From<CompletionShell> for usage::complete::Shell {
    fn from(shell: CompletionShell) -> Self {
        match shell {
            CompletionShell::Bash => Self::Bash,
            CompletionShell::Zsh => Self::Zsh,
            CompletionShell::Fish => Self::Fish,
            CompletionShell::Nu => Self::Nu,
            CompletionShell::PowerShell => Self::PowerShell,
        }
    }
}

/// Run the command-line process.
#[must_use]
pub fn run() -> ExitCode {
    let argv = env::args_os().collect::<Vec<_>>();
    let current_directory = env::current_dir();
    let mut stdout = io::stdout().lock();
    // Embedded libraries may write to the process stderr directly. Keeping the
    // global lock for the whole check would deadlock when they do.
    let mut stderr = io::stderr();
    run_with_argv(&argv, current_directory, &mut stdout, &mut stderr)
}

fn run_with_argv<W>(
    argv: &[OsString],
    current_directory: io::Result<PathBuf>,
    stdout: &mut dyn Write,
    stderr: &mut W,
) -> ExitCode
where
    W: anstream::stream::RawStream + anstream::stream::AsLockedWrite + ?Sized,
{
    let completion_argv = argv.iter().skip(1).cloned().collect::<Vec<_>>();
    if let Some(answer) = Arguments::completion_request(&completion_argv) {
        return match stdout.write_all(answer.as_bytes()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let _ = writeln!(stderr, "error: {error}");
                ExitCode::from(OPERATIONAL_ERROR_CODE)
            }
        };
    }

    let argv_refs = argv.iter().map(OsString::as_os_str).collect::<Vec<_>>();
    let arguments = match Arguments::parse_from_argv(&argv_refs) {
        Ok(arguments) => arguments,
        Err(usage::Error::Help { cmd, long }) => {
            if let Some(page) = usage::help::render_styled(Arguments::spec(), cmd, long, usage::help::Style::auto()) {
                let _ = stdout.write_all(page.as_bytes());
            }
            return ExitCode::SUCCESS;
        }
        Err(usage::Error::HelpAll { cmd }) => {
            if let Some(page) = usage::help::render_all_styled(Arguments::spec(), cmd, usage::help::Style::auto()) {
                let _ = stdout.write_all(page.as_bytes());
            }
            return ExitCode::SUCCESS;
        }
        Err(usage::Error::Version { .. }) => {
            let _ = writeln!(stdout, "legato {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(usage::Error::MissingArgsHelp { cmd }) => {
            if let Some(page) =
                usage::help::render_styled(Arguments::spec(), cmd, false, usage::help::Style::auto_stderr())
            {
                let _ = stderr.write_all(page.as_bytes());
            }
            return ExitCode::from(OPERATIONAL_ERROR_CODE);
        }
        Err(error) => {
            let words = argv_refs.get(1..).unwrap_or_default();
            let message = Arguments::render_failure(words, &error);
            let _ = stderr.write_all(message.as_bytes());
            return ExitCode::from(OPERATIONAL_ERROR_CODE);
        }
    };

    let mut stderr = AutoStream::new(stderr, arguments.color.into());

    if let Some(Command::Completion(completion)) = arguments.command.as_ref() {
        let script = Arguments::completion_script(completion.shell.into());
        return match stdout.write_all(script.as_bytes()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let _ = writeln!(stderr, "{DANGER_STYLE}error: {error}{DANGER_STYLE:#}");
                ExitCode::from(OPERATIONAL_ERROR_CODE)
            }
        };
    }

    let current_directory = match current_directory {
        Ok(current_directory) => current_directory,
        Err(error) => {
            let _ = writeln!(stderr, "{DANGER_STYLE}error: {error}{DANGER_STYLE:#}");
            return ExitCode::from(OPERATIONAL_ERROR_CODE);
        }
    };
    match execute(arguments, current_directory, stdout, &mut stderr) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(CHANGES_DETECTED_CODE),
        Err(error) => {
            let _ = writeln!(stderr, "{DANGER_STYLE}error: {error}{DANGER_STYLE:#}");
            ExitCode::from(OPERATIONAL_ERROR_CODE)
        }
    }
}

fn execute(
    arguments: Arguments,
    repository_path: PathBuf,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<usize, CheckError> {
    let human_output = arguments.format.contains(&OutputFormat::Console);
    let options = CheckOptions {
        repository: repository_path.clone(),
        from_revision: arguments.from.flatten(),
        to_revision: arguments.to,
        install_development_dependencies: arguments.install_development_dependencies,
    };

    let mut observer_error = None;
    let report = {
        let mut callback = |message: &str| {
            if observer_error.is_none()
                && let Err(error) = writeln!(stderr, "{MUTED_STYLE}{message}{MUTED_STYLE:#}")
            {
                observer_error = Some(error);
            }
        };
        let result = if human_output {
            repository::check(options, Observer::new(&mut callback))
        } else {
            repository::check(options, Observer::default())
        };
        if let Some(error) = observer_error {
            return Err(error.into());
        }
        result?
    };

    if human_output {
        writeln!(stderr)?;
    }
    output::write(&arguments.format, &report, &repository_path, true, stdout, stderr)?;
    let breaking_changes = report.changes.iter().filter(|change| change.is_breaking()).count();
    Ok(breaking_changes)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn argv(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_defaults() {
        let words = argv(&["legato"]);
        let refs = words.iter().map(OsString::as_os_str).collect::<Vec<_>>();
        assert_eq!(
            Arguments::parse_from_argv(&refs).unwrap(),
            Arguments {
                from: None,
                to: "HEAD".to_owned(),
                format: vec![OutputFormat::Console],
                color: ColorMode::Auto,
                install_development_dependencies: false,
                command: None,
            }
        );
    }

    #[test]
    fn parses_optional_from_and_repeatable_optional_formats() {
        let words = argv(&[
            "legato",
            "--from",
            "--to=next",
            "--format=json",
            "--format",
            "--install-development-dependencies",
        ]);
        let refs = words.iter().map(OsString::as_os_str).collect::<Vec<_>>();
        assert_eq!(
            Arguments::parse_from_argv(&refs).unwrap(),
            Arguments {
                from: Some(None),
                to: "next".to_owned(),
                format: vec![OutputFormat::Json, OutputFormat::Console],
                color: ColorMode::Auto,
                install_development_dependencies: true,
                command: None,
            }
        );

        let words = argv(&["legato", "--from=v1.2.3"]);
        let refs = words.iter().map(OsString::as_os_str).collect::<Vec<_>>();
        assert_eq!(
            Arguments::parse_from_argv(&refs).unwrap().from,
            Some(Some("v1.2.3".to_owned()))
        );
    }

    #[test]
    fn parses_explicit_and_flag_only_color_modes() {
        for (arguments, expected) in [
            (&["legato", "--color=never"][..], ColorMode::Never),
            (&["legato", "--color=always"][..], ColorMode::Always),
            (&["legato", "--color"][..], ColorMode::Always),
        ] {
            let words = argv(arguments);
            let refs = words.iter().map(OsString::as_os_str).collect::<Vec<_>>();
            assert_eq!(Arguments::parse_from_argv(&refs).unwrap().color, expected);
        }
    }

    #[test]
    fn explicit_color_modes_are_applied_by_the_output_stream() {
        for (mode, colored) in [("always", true), ("never", false)] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let status = run_with_argv(
                &argv(&["legato", &format!("--color={mode}")]),
                Err(io::Error::new(io::ErrorKind::NotFound, "missing cwd")),
                &mut stdout,
                &mut stderr,
            );

            assert_eq!(status, ExitCode::from(OPERATIONAL_ERROR_CODE));
            assert_eq!(stderr.contains(&0x1b), colored, "{mode}: {stderr:?}");
        }
    }

    #[test]
    fn invalid_arguments_are_operational_errors() {
        let directory = TempDir::new().unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let status = run_with_argv(
            &argv(&["legato", "--unknown"]),
            Ok(directory.path().to_path_buf()),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(status, ExitCode::from(OPERATIONAL_ERROR_CODE));
        assert!(stdout.is_empty());
        assert!(String::from_utf8(stderr).unwrap().contains("--unknown"));
    }

    #[test]
    fn help_and_version_succeed_without_accessing_the_repository() {
        for (argument, expected) in [("--help", "Usage: legato"), ("--version", "legato 0.1.0")] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let status = run_with_argv(
                &argv(&["legato", argument]),
                Err(io::Error::new(io::ErrorKind::NotFound, "missing cwd")),
                &mut stdout,
                &mut stderr,
            );
            assert_eq!(status, ExitCode::SUCCESS);
            assert!(String::from_utf8(stdout).unwrap().contains(expected));
            assert!(stderr.is_empty());
        }
    }

    #[test]
    fn rejects_unknown_output_formats() {
        let words = argv(&["legato", "--format=sarif"]);
        let refs = words.iter().map(OsString::as_os_str).collect::<Vec<_>>();
        assert!(Arguments::parse_from_argv(&refs).is_err());
    }

    #[test]
    fn generates_completion_scripts_without_accessing_the_repository() {
        for shell in ["bash", "zsh", "fish", "nu", "powershell", "nushell", "pwsh"] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let status = run_with_argv(
                &argv(&["legato", "completion", shell]),
                Err(io::Error::new(io::ErrorKind::NotFound, "missing cwd")),
                &mut stdout,
                &mut stderr,
            );

            assert_eq!(status, ExitCode::SUCCESS, "{shell}");
            let script = String::from_utf8(stdout).unwrap();
            assert!(script.contains("legato"), "{shell}: {script}");
            assert!(script.contains("__complete_word__"), "{shell}: {script}");
            assert!(stderr.is_empty(), "{shell}: {}", String::from_utf8_lossy(&stderr));
        }
    }

    #[test]
    fn completion_protocol_offers_flags_and_declared_values() {
        let flags = Arguments::completion_request(&[
            "__complete_word__".into(),
            "--shell".into(),
            "zsh".into(),
            "--line".into(),
            "legato --for".into(),
        ])
        .unwrap();
        assert!(flags.contains("--format"), "{flags}");

        let shells = Arguments::completion_request(&[
            "__complete_word__".into(),
            "--shell".into(),
            "zsh".into(),
            "--line".into(),
            "legato completion ".into(),
        ])
        .unwrap();
        for shell in ["bash", "zsh", "fish", "nu", "powershell"] {
            assert!(shells.contains(shell), "missing {shell}: {shells}");
        }
    }
}
