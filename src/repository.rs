use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use semver::Version;
use tempfile::{Builder as TempDirBuilder, TempDir};

use crate::composer;
use crate::config::{CONFIGURATION_FILENAME, Configuration};
use crate::snapshot::{Snapshot, SourceFile};
use crate::{CheckError, CheckOptions, CheckReport};

/// Receives progress messages without coupling the library entry point to a terminal.
#[derive(Default)]
pub(crate) struct Observer<'a> {
    callback: Option<&'a mut dyn FnMut(&str)>,
}

impl<'a> Observer<'a> {
    pub(crate) fn new(callback: &'a mut dyn FnMut(&str)) -> Self {
        Self {
            callback: Some(callback),
        }
    }

    fn status(&mut self, message: impl AsRef<str>) {
        if let Some(callback) = self.callback.as_mut() {
            callback(message.as_ref());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Revision {
    reference: String,
    sha: String,
    branch: Option<String>,
}

impl Revision {
    #[must_use]
    #[cfg(test)]
    pub(crate) fn reference(&self) -> &str {
        &self.reference
    }

    #[must_use]
    pub(crate) fn sha(&self) -> &str {
        &self.sha
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Repository {
    root: PathBuf,
}

impl Repository {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self, CheckError> {
        let requested = path.as_ref();
        let candidate =
            fs::canonicalize(requested).map_err(|_| CheckError::InvalidRepository(requested.to_path_buf()))?;

        let inside = Command::new("git")
            .arg("-C")
            .arg(&candidate)
            .args(["rev-parse", "--is-inside-work-tree"])
            .output()
            .map_err(|_| CheckError::InvalidRepository(requested.to_path_buf()))?;
        if !inside.status.success() || trim_ascii(&inside.stdout) != b"true" {
            return Err(CheckError::InvalidRepository(requested.to_path_buf()));
        }

        let top_level = git(&candidate, [OsStr::new("rev-parse"), OsStr::new("--show-toplevel")])?;
        let root = path_from_git_output(&top_level.stdout);
        let root = fs::canonicalize(&root).map_err(CheckError::from)?;
        Ok(Self { root })
    }

    #[must_use]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn resolve(&self, reference: &str) -> Result<Revision, CheckError> {
        if reference.is_empty() {
            return Err(CheckError::Git("revision must not be empty".to_owned()));
        }

        let peeled = format!("{reference}^{{commit}}");
        let output = git(
            &self.root,
            [
                OsStr::new("rev-parse"),
                OsStr::new("--verify"),
                OsStr::new("--end-of-options"),
                OsStr::new(&peeled),
            ],
        )?;
        let sha = String::from_utf8_lossy(trim_ascii(&output.stdout)).into_owned();
        if !is_object_id(&sha) {
            return Err(CheckError::Git(format!(
                "Git returned an invalid commit id for `{reference}`: `{sha}`"
            )));
        }

        let branch = if reference.starts_with('-') {
            None
        } else {
            let symbolic = git(
                &self.root,
                [
                    OsStr::new("rev-parse"),
                    OsStr::new("--verify"),
                    OsStr::new("--symbolic-full-name"),
                    OsStr::new(reference),
                ],
            )?;
            let symbolic = String::from_utf8_lossy(trim_ascii(&symbolic.stdout));
            symbolic.strip_prefix("refs/heads/").map(str::to_owned)
        };

        Ok(Revision {
            reference: reference.to_owned(),
            sha,
            branch,
        })
    }

    pub(crate) fn latest_stable_tag(&self) -> Result<String, CheckError> {
        let output = git(&self.root, [OsStr::new("tag"), OsStr::new("--list")])?;
        let mut versions = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|tag| {
                let version = tag
                    .strip_prefix('v')
                    .or_else(|| tag.strip_prefix("release-"))
                    .unwrap_or(tag);
                Version::parse(version)
                    .ok()
                    .filter(|version| version.pre.is_empty())
                    .map(|version| (version, tag.to_owned()))
            })
            .collect::<Vec<_>>();
        versions.sort_by(|(left_version, _), (right_version, _)| right_version.cmp_precedence(left_version));
        versions.into_iter().next().map(|(_, tag)| tag).ok_or_else(|| {
            CheckError::Git("Could not detect any released versions for the given repository".to_owned())
        })
    }

    fn checkout(&self, revision: &Revision, label: &str) -> Result<Checkout, CheckError> {
        let directory = TempDirBuilder::new().prefix(&format!("legato-{label}-")).tempdir()?;
        let clone = Command::new("git")
            .arg("clone")
            .arg("--quiet")
            .arg("--no-hardlinks")
            .arg("--no-checkout")
            .arg("--config")
            .arg("core.hooksPath=/dev/null")
            .arg("--")
            .arg(&self.root)
            .arg(directory.path())
            .output()
            .map_err(|error| CheckError::Git(format!("unable to execute `git clone`: {error}")))?;
        ensure_git_success("clone repository", clone)?;

        let checkout = if let Some(branch) = revision.branch.as_deref() {
            git(
                directory.path(),
                [
                    OsStr::new("checkout"),
                    OsStr::new("--quiet"),
                    OsStr::new("--force"),
                    OsStr::new("-B"),
                    OsStr::new(branch),
                    OsStr::new(revision.sha()),
                    OsStr::new("--"),
                ],
            )?
        } else {
            git(
                directory.path(),
                [
                    OsStr::new("checkout"),
                    OsStr::new("--quiet"),
                    OsStr::new("--detach"),
                    OsStr::new("--force"),
                    OsStr::new(revision.sha()),
                    OsStr::new("--"),
                ],
            )?
        };
        debug_assert!(checkout.status.success());

        Ok(Checkout {
            directory,
            revision: revision.clone(),
        })
    }
}

#[derive(Debug)]
pub(crate) struct Checkout {
    directory: TempDir,
    revision: Revision,
}

impl Checkout {
    #[must_use]
    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }

    #[must_use]
    pub(crate) fn revision(&self) -> &Revision {
        &self.revision
    }
}

/// Two detached repository copies kept alive for the duration of a comparison.
/// Dropping this value removes both directories, including on every error path.
#[derive(Debug)]
pub(crate) struct PreparedComparison {
    from: Checkout,
    to: Checkout,
}

#[derive(Debug)]
struct ResolvedComparison {
    repository: Repository,
    from_revision: Revision,
    to_revision: Revision,
}

impl PreparedComparison {
    #[must_use]
    pub(crate) fn from(&self) -> &Checkout {
        &self.from
    }

    #[must_use]
    pub(crate) fn to(&self) -> &Checkout {
        &self.to
    }
}

#[cfg(test)]
fn prepare(options: &CheckOptions, observer: &mut Observer<'_>) -> Result<PreparedComparison, CheckError> {
    checkout_comparison(resolve_comparison(options, observer)?, observer)
}

fn resolve_comparison(options: &CheckOptions, observer: &mut Observer<'_>) -> Result<ResolvedComparison, CheckError> {
    let repository = Repository::open(&options.repository)?;
    let from_reference = match options.from_revision.as_deref() {
        None => {
            let tag = repository.latest_stable_tag()?;
            observer.status(format!("Detected last version: {tag}"));
            tag
        }
        Some(reference) => reference.to_owned(),
    };
    let from_revision = repository.resolve(&from_reference)?;
    let to_revision = repository.resolve(&options.to_revision)?;
    Ok(ResolvedComparison {
        repository,
        from_revision,
        to_revision,
    })
}

fn checkout_comparison(
    resolved: ResolvedComparison,
    observer: &mut Observer<'_>,
) -> Result<PreparedComparison, CheckError> {
    let ResolvedComparison {
        repository,
        from_revision,
        to_revision,
    } = resolved;
    observer.status(format!(
        "Comparing from {} to {}...",
        from_revision.sha(),
        to_revision.sha()
    ));

    let (from, to) = std::thread::scope(|scope| {
        let from = scope.spawn(|| repository.checkout(&from_revision, "from"));
        let to = scope.spawn(|| repository.checkout(&to_revision, "to"));
        let from = from.join();
        let to = to.join();
        let from = from.map_err(|_| CheckError::Git("the base checkout worker panicked".to_owned()))??;
        let to = to.map_err(|_| CheckError::Git("the target checkout worker panicked".to_owned()))??;
        Ok::<_, CheckError>((from, to))
    })?;
    Ok(PreparedComparison { from, to })
}

/// Execute the complete repository check.
///
/// The orchestration body is kept here so `PreparedComparison` owns both temporary checkouts
/// until snapshots and comparison have finished.
pub(crate) fn check(options: CheckOptions, mut observer: Observer<'_>) -> Result<CheckReport, CheckError> {
    let resolved = resolve_comparison(&options, &mut observer)?;
    let configuration = Configuration::load(resolved.repository.root())?;
    if let Some(filename) = configuration.filename.as_ref() {
        observer.status(format!("Using \"{}\" as configuration file", filename.display()));
    } else {
        observer.status(format!(
            "No \"{CONFIGURATION_FILENAME}\" found; using PHP {} with all extensions",
            configuration.platform.php()
        ));
    }
    let prepared = checkout_comparison(resolved, &mut observer)?;
    check_prepared(options, prepared, configuration)
}

fn check_prepared(
    options: CheckOptions,
    prepared: PreparedComparison,
    configuration: Configuration,
) -> Result<CheckReport, CheckError> {
    let php_version = configuration.platform.php();
    let session = riff_core::RiffSession::new()
        .map_err(|error| CheckError::Composer(format!("unable to initialize Riff session: {error:#}")))?;
    let (from_installation, to_installation) = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
                runtime.block_on(async {
                    let from = composer::prepare_install(
                        &session,
                        prepared.from().path(),
                        options.install_development_dependencies,
                        php_version,
                    )?;
                    let to = composer::prepare_install(
                        &session,
                        prepared.to().path(),
                        options.install_development_dependencies,
                        php_version,
                    )?;
                    let (from_request, from_plan) = from.into_request_and_plan();
                    let (to_request, to_plan) = to.into_request_and_plan();
                    let mut results = session
                        .install_projects([from_request, to_request], riff_core::BatchOptions::default())
                        .await
                        .into_iter();
                    let from = from_plan.finish(results.next().expect("from result"))?;
                    let to = to_plan.finish(results.next().expect("to result"))?;
                    Ok::<_, CheckError>((from, to))
                })
            })
            .join()
            .map_err(|_| CheckError::Composer("the Riff install worker panicked".to_owned()))?
    })?;
    let from_snapshot = build_snapshot(&from_installation, &configuration)?;
    let to_snapshot = build_snapshot(&to_installation, &configuration)?;
    let changes = crate::compare::compare(&from_snapshot, &to_snapshot);
    let changes = configuration.baseline.filter(&changes);

    Ok(CheckReport {
        from_revision: prepared.from().revision().sha().to_owned(),
        to_revision: prepared.to().revision().sha().to_owned(),
        changes,
    })
}

fn build_snapshot(
    installation: &composer::Installation,
    configuration: &Configuration,
) -> Result<Snapshot, CheckError> {
    let root = installation.root();
    build_snapshot_from_sources(
        root,
        installation.project_sources(),
        installation.dependency_sources(),
        configuration,
    )
}

fn build_snapshot_from_sources(
    root: &Path,
    project_sources: &[PathBuf],
    dependency_sources: &[PathBuf],
    configuration: &Configuration,
) -> Result<Snapshot, CheckError> {
    let sources = project_sources
        .iter()
        .cloned()
        .map(|path| SourceFile::project(root, path))
        .chain(
            dependency_sources
                .iter()
                .cloned()
                .map(|path| SourceFile::dependency(root, path)),
        )
        .collect::<Vec<_>>();
    Snapshot::build_with_configuration(root, sources, configuration)
}

fn git<I, S>(directory: &Path, arguments: I) -> Result<Output, CheckError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .map_err(|error| CheckError::Git(format!("unable to execute Git: {error}")))?;
    ensure_git_success("run Git command", output)
}

fn ensure_git_success(operation: &str, output: Output) -> Result<Output, CheckError> {
    if output.status.success() {
        return Ok(output);
    }

    let status = output
        .status
        .code()
        .map_or_else(|| "terminated by signal".to_owned(), |code| code.to_string());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(CheckError::Git(format!(
        "unable to {operation} (status {status})\nstdout:\n{stdout}\nstderr:\n{stderr}"
    )))
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn path_from_git_output(bytes: &[u8]) -> PathBuf {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;

        PathBuf::from(OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use tempfile::TempDir;

    use super::*;

    fn command(directory: &Path, program: &str, arguments: &[&str]) {
        let status = Command::new(program)
            .current_dir(directory)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{program} {arguments:?}");
    }

    fn repository() -> (TempDir, String, String) {
        let directory = TempDir::new().unwrap();
        command(directory.path(), "git", &["init", "--quiet"]);
        command(directory.path(), "git", &["config", "user.email", "test@example.com"]);
        command(directory.path(), "git", &["config", "user.name", "Test"]);
        command(directory.path(), "git", &["config", "tag.gpgSign", "false"]);

        fs::write(directory.path().join("composer.json"), "{}").unwrap();
        fs::write(
            directory.path().join(crate::config::CONFIGURATION_FILENAME),
            "[platform]\nphp = \"8.5.9\"\nextensions = \"all\"\n",
        )
        .unwrap();
        command(directory.path(), "git", &["add", "composer.json", "legato.toml"]);
        command(directory.path(), "git", &["commit", "--quiet", "-m", "first"]);
        let first = String::from_utf8(
            Command::new("git")
                .current_dir(directory.path())
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();

        fs::write(directory.path().join("source.php"), "<?php class Current {}\n").unwrap();
        command(directory.path(), "git", &["add", "source.php"]);
        command(directory.path(), "git", &["commit", "--quiet", "-m", "second"]);
        let second = String::from_utf8(
            Command::new("git")
                .current_dir(directory.path())
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        (directory, first, second)
    }

    #[test]
    fn validates_repository_and_resolves_commits() {
        let (directory, first, second) = repository();
        let repository = Repository::open(directory.path()).unwrap();
        let branch = git(
            directory.path(),
            [OsStr::new("symbolic-ref"), OsStr::new("--short"), OsStr::new("HEAD")],
        )
        .unwrap();
        let branch = String::from_utf8_lossy(trim_ascii(&branch.stdout));
        assert_eq!(repository.root(), fs::canonicalize(directory.path()).unwrap());
        assert_eq!(repository.resolve(&first[..8]).unwrap().sha(), first);
        assert_eq!(repository.resolve(&first[..8]).unwrap().branch(), None);
        let head = repository.resolve("HEAD").unwrap();
        assert_eq!(head.sha(), second);
        assert_eq!(head.branch(), Some(branch.as_ref()));

        let not_repository = TempDir::new().unwrap();
        assert!(matches!(
            Repository::open(not_repository.path()),
            Err(CheckError::InvalidRepository(path)) if path == not_repository.path()
        ));
    }

    #[test]
    fn preserves_symbolic_branch_context_in_the_checkout() {
        let (directory, _, _) = repository();
        let repository = Repository::open(directory.path()).unwrap();
        let revision = repository.resolve("HEAD").unwrap();
        let expected = revision.branch().unwrap().to_owned();
        let checkout = repository.checkout(&revision, "branch-test").unwrap();
        let branch = git(
            checkout.path(),
            [OsStr::new("symbolic-ref"), OsStr::new("--short"), OsStr::new("HEAD")],
        )
        .unwrap();

        assert_eq!(String::from_utf8_lossy(trim_ascii(&branch.stdout)), expected);
        assert_eq!(checkout.revision().sha(), revision.sha());
    }

    #[test]
    fn preserves_trailing_spaces_in_repository_paths() {
        let parent = TempDir::new().unwrap();
        let directory = parent.path().join("repository ");
        fs::create_dir(&directory).unwrap();
        command(&directory, "git", &["init", "--quiet"]);
        assert_eq!(
            Repository::open(&directory).unwrap().root(),
            fs::canonicalize(&directory).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserves_non_utf8_repository_paths() {
        use std::os::unix::ffi::OsStringExt;

        let parent = TempDir::new().unwrap();
        let directory = parent.path().join(OsString::from_vec(b"repository-\xff".to_vec()));
        fs::create_dir(&directory).unwrap();
        command(&directory, "git", &["init", "--quiet"]);
        assert_eq!(
            Repository::open(&directory).unwrap().root(),
            fs::canonicalize(&directory).unwrap()
        );
    }

    #[test]
    fn selects_highest_stable_semver_tag_with_upstream_prefixes() {
        let (directory, _, _) = repository();
        for tag in [
            "1.2.3",
            "v2.0.0",
            "release-2.1.0",
            "9.0.0-beta.1",
            "release-10",
            "v1.20.0",
            "V99.0.0",
        ] {
            command(directory.path(), "git", &["tag", tag]);
        }
        assert_eq!(
            Repository::open(directory.path()).unwrap().latest_stable_tag().unwrap(),
            "release-2.1.0"
        );
    }

    #[test]
    fn equal_precedence_tags_keep_git_listing_order() {
        let (directory, _, _) = repository();
        command(directory.path(), "git", &["tag", "1.0.0+z"]);
        command(directory.path(), "git", &["tag", "1.0.0+a"]);
        assert_eq!(
            Repository::open(directory.path()).unwrap().latest_stable_tag().unwrap(),
            "1.0.0+a"
        );
    }

    #[test]
    fn rejects_a_repository_without_stable_version_tags() {
        let (directory, _, _) = repository();
        command(directory.path(), "git", &["tag", "1.0.0-rc.1"]);
        let error = Repository::open(directory.path())
            .unwrap()
            .latest_stable_tag()
            .unwrap_err()
            .to_string();
        assert!(error.contains("Could not detect any released versions"), "{error}");
    }

    #[test]
    fn prepares_isolated_checkouts_and_removes_them_on_drop() {
        let (directory, first, second) = repository();
        let options = CheckOptions {
            repository: directory.path().to_path_buf(),
            from_revision: Some(first.clone()),
            to_revision: second.clone(),
            install_development_dependencies: false,
        };
        let mut messages = Vec::new();
        let mut callback = |message: &str| messages.push(message.to_owned());
        let prepared = prepare(&options, &mut Observer::new(&mut callback)).unwrap();
        let from_path = prepared.from().path().to_path_buf();
        let to_path = prepared.to().path().to_path_buf();

        assert_ne!(from_path, to_path);
        assert_ne!(from_path, directory.path());
        assert!(!from_path.join("source.php").exists());
        assert!(to_path.join("source.php").exists());
        assert_eq!(prepared.from().revision().sha(), first);
        assert_eq!(prepared.to().revision().sha(), second);
        assert!(messages.iter().any(|message| message.starts_with("Comparing from ")));

        drop(prepared);
        assert!(!from_path.exists());
        assert!(!to_path.exists());
    }

    #[test]
    fn bare_from_selects_the_latest_tag() {
        let (directory, _, second) = repository();
        command(directory.path(), "git", &["tag", "v1.0.0"]);

        let options = CheckOptions {
            repository: directory.path().to_path_buf(),
            from_revision: None,
            to_revision: "HEAD".to_owned(),
            install_development_dependencies: false,
        };
        let prepared = prepare(&options, &mut Observer::default()).unwrap();
        assert_eq!(prepared.from().revision().reference(), "v1.0.0");
        assert_eq!(prepared.to().revision().sha(), second);
    }

    #[test]
    fn validates_configuration_before_reporting_or_creating_checkouts() {
        let (directory, first, second) = repository();
        fs::write(
            directory.path().join(crate::config::CONFIGURATION_FILENAME),
            "<invalid />",
        )
        .unwrap();
        let options = CheckOptions {
            repository: directory.path().to_path_buf(),
            from_revision: Some(first),
            to_revision: second,
            install_development_dependencies: false,
        };
        let mut messages = Vec::new();
        let mut callback = |message: &str| messages.push(message.to_owned());

        let error = check(options, Observer::new(&mut callback)).unwrap_err();
        assert!(matches!(error, CheckError::Configuration(_)));
        assert!(!messages.iter().any(|message| message.starts_with("Comparing from ")));
    }

    #[test]
    fn applies_project_path_exclusions_when_building_repository_snapshots() {
        let directory = TempDir::new().unwrap();
        let project = directory.path().join("src");
        let internal = project.join("Internal");
        let dependency = directory.path().join("vendor/acme/package/src");
        fs::create_dir_all(&internal).unwrap();
        fs::create_dir_all(&dependency).unwrap();
        fs::write(project.join("Api.php"), "<?php namespace Acme; class Api {}\n").unwrap();
        fs::write(
            internal.join("Generated.php"),
            "<?php namespace Acme; class Generated {}\n",
        )
        .unwrap();
        fs::write(
            directory.path().join("bootstrap.inc"),
            "<?php namespace Acme; class Bootstrap {}\n",
        )
        .unwrap();
        fs::write(
            dependency.join("Context.php"),
            "<?php namespace Vendor; class Context {}\n",
        )
        .unwrap();
        fs::write(
            directory.path().join(crate::config::CONFIGURATION_FILENAME),
            r#"[platform]
php = "8.5.9"
extensions = "all"

[paths]
exclude = ["src/Internal/**", "bootstrap.inc", "vendor/**"]
"#,
        )
        .unwrap();
        let configuration = Configuration::load(directory.path()).unwrap();

        let snapshot = build_snapshot_from_sources(
            directory.path(),
            &[project, directory.path().join("bootstrap.inc")],
            &[dependency],
            &configuration,
        )
        .unwrap();

        assert!(snapshot.class_like("Acme\\Api").is_some());
        assert!(snapshot.class_like("Acme\\Generated").is_none());
        assert!(snapshot.class_like("Acme\\Bootstrap").is_none());
        assert_eq!(
            snapshot.class_like("Vendor\\Context").map(|class_like| class_like.role),
            Some(crate::snapshot::SourceRole::Dependency)
        );
    }
}
