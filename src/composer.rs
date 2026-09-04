use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use riff_core::config::Config;
use riff_core::installer::{InstallOptions, PlatformRequirementFilter, UpdateOptions};
use riff_core::json::{RiffLockfile, RiffManifest};
use riff_core::{Platform, ProjectInstallRequest, ProjectInstallResult, RiffSession};
use serde::Deserialize;

use crate::CheckError;

const COMPOSER_JSON: &str = "composer.json";
const DEFAULT_VENDOR_DIR: &str = "vendor";

/// The source roots made visible by one Composer installation.
///
/// Project sources intentionally come from the root package's `autoload` section only.
/// Dependencies are context for resolving referenced symbols, not part of the API being
/// compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Installation {
    root: PathBuf,
    project_sources: Vec<PathBuf>,
    dependency_sources: Vec<PathBuf>,
}

impl Installation {
    #[must_use]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub(crate) fn project_sources(&self) -> &[PathBuf] {
        &self.project_sources
    }

    #[must_use]
    pub(crate) fn dependency_sources(&self) -> &[PathBuf] {
        &self.dependency_sources
    }
}

/// A configured project waiting to be submitted to Riff's batch installer.
pub(crate) struct PendingInstallation {
    request: ProjectInstallRequest,
    root: PathBuf,
    project_sources: Vec<PathBuf>,
    vendor_dir: PathBuf,
    include_development_dependencies: bool,
}

impl PendingInstallation {
    pub(crate) fn into_request_and_plan(self) -> (ProjectInstallRequest, InstallationPlan) {
        (
            self.request,
            InstallationPlan {
                root: self.root,
                project_sources: self.project_sources,
                vendor_dir: self.vendor_dir,
                include_development_dependencies: self.include_development_dependencies,
            },
        )
    }
}

pub(crate) struct InstallationPlan {
    root: PathBuf,
    project_sources: Vec<PathBuf>,
    vendor_dir: PathBuf,
    include_development_dependencies: bool,
}

impl InstallationPlan {
    pub(crate) fn finish(self, result: ProjectInstallResult) -> Result<Installation, CheckError> {
        let exit_code = result
            .into_result()
            .map_err(|error| CheckError::Composer(format!("Riff dependency installation failed: {error:#}")))?;
        if exit_code != 0 {
            return Err(CheckError::Composer(format!(
                "Riff dependency installation exited with status {exit_code}"
            )));
        }
        let dependency_sources =
            dependency_sources(&self.root, &self.vendor_dir, self.include_development_dependencies)?;
        Ok(Installation {
            root: self.root,
            project_sources: self.project_sources,
            dependency_sources,
        })
    }
}

/// Configure one project for a shared Riff batch installation.
pub(crate) fn prepare_install(
    session: &RiffSession,
    root: &Path,
    include_development_dependencies: bool,
    php_version: &str,
) -> Result<PendingInstallation, CheckError> {
    let manifest = read_manifest(root)?;
    let vendor_dir = resolve_vendor_dir(
        root,
        manifest.config.as_ref().and_then(|config| config.vendor_dir.as_deref()),
    )?;
    let project_sources = autoload_sources(root, root, &manifest.autoload)?;
    let request = install_with_riff(
        session,
        root,
        &vendor_dir,
        include_development_dependencies,
        php_version,
    )?;

    Ok(PendingInstallation {
        request,
        root: root.to_path_buf(),
        project_sources,
        vendor_dir,
        include_development_dependencies,
    })
}

fn install_with_riff(
    session: &RiffSession,
    root: &Path,
    vendor_dir: &Path,
    include_development_dependencies: bool,
    php_version: &str,
) -> Result<ProjectInstallRequest, CheckError> {
    let manifest_path = root.join(COMPOSER_JSON);
    let manifest_contents = fs::read_to_string(&manifest_path)
        .map_err(|error| CheckError::Composer(format!("could not read {}: {error}", manifest_path.display())))?;
    let riff_manifest: RiffManifest = serde_json::from_str(&manifest_contents)?;
    let lock_path = root.join("composer.lock");
    let lockfile = match fs::read_to_string(&lock_path) {
        Ok(contents) => Some(serde_json::from_str::<RiffLockfile>(&contents)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(CheckError::Composer(format!(
                "could not read {}: {error}",
                lock_path.display()
            )));
        }
    };
    let has_lockfile = lockfile.is_some();

    let relative_vendor_dir = vendor_dir.strip_prefix(root).map_err(|_| {
        CheckError::Composer(format!(
            "vendor directory {} is outside the temporary checkout",
            vendor_dir.display()
        ))
    })?;
    let mut config = Config::with_base_dir(root);
    config.vendor_dir = relative_vendor_dir.to_path_buf();
    config.bin_dir = relative_vendor_dir.join("bin");
    config.policy = serde_json::Value::Bool(false);
    config.audit_policy = serde_json::Value::Bool(false);

    let platform = Platform::empty().with_package("php", php_version);
    let riff = session
        .project(root.to_path_buf())
        .with_config(config)
        .with_manifest(riff_manifest)
        .with_lockfile(lockfile)
        .with_platform(platform)
        .plugins_enabled(false)
        .prefer_dist(true)
        .no_dev(!include_development_dependencies)
        .build()
        .map_err(|error| CheckError::Composer(format!("unable to initialize Riff: {error:#}")))?;
    let platform_filter = PlatformRequirementFilter {
        all: false,
        requirements: vec!["ext-*".to_owned(), "lib-*".to_owned(), "php-*".to_owned()],
    };
    let request = if has_lockfile {
        ProjectInstallRequest::install(
            riff,
            InstallOptions {
                ignore_platform_requirements: platform_filter,
                no_autoloader: true,
                no_scripts: true,
                no_blocking: true,
                ..Default::default()
            },
        )
    } else {
        ProjectInstallRequest::update(
            riff,
            UpdateOptions {
                ignore_platform_requirements: platform_filter,
                no_autoloader: true,
                no_scripts: true,
                no_blocking: true,
                ..Default::default()
            },
        )
    };
    Ok(request)
}

#[derive(Debug, Default, Deserialize)]
struct ComposerManifest {
    #[serde(default)]
    autoload: Autoload,
    #[serde(default)]
    config: Option<ComposerConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct ComposerConfig {
    #[serde(rename = "vendor-dir", default)]
    vendor_dir: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Autoload {
    #[serde(rename = "psr-0", default)]
    psr_0: BTreeMap<String, OneOrMany>,
    #[serde(rename = "psr-4", default)]
    psr_4: BTreeMap<String, OneOrMany>,
    #[serde(default)]
    classmap: OneOrMany,
    #[serde(default)]
    files: OneOrMany,
}

#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
    #[default]
    None,
}

impl OneOrMany {
    fn values(&self) -> impl Iterator<Item = &str> {
        let values: &[String] = match self {
            Self::One(value) => std::slice::from_ref(value),
            Self::Many(values) => values,
            Self::None => &[],
        };
        values.iter().map(String::as_str)
    }
}

fn read_manifest(root: &Path) -> Result<ComposerManifest, CheckError> {
    let path = root.join(COMPOSER_JSON);
    let contents = fs::read_to_string(&path)
        .map_err(|error| CheckError::Composer(format!("could not read {}: {error}", path.display())))?;
    serde_json::from_str(&contents).map_err(CheckError::from)
}

fn resolve_vendor_dir(root: &Path, configured: Option<&str>) -> Result<PathBuf, CheckError> {
    let configured = configured.unwrap_or(DEFAULT_VENDOR_DIR);
    resolve_within(root, root, configured, false)
        .map_err(|reason| CheckError::Composer(format!("invalid Composer vendor-dir `{configured}`: {reason}")))
}

fn autoload_sources(root: &Path, package_root: &Path, autoload: &Autoload) -> Result<Vec<PathBuf>, CheckError> {
    let mut raw_paths = Vec::new();
    raw_paths.extend(autoload.psr_0.values().flat_map(OneOrMany::values));
    raw_paths.extend(autoload.psr_4.values().flat_map(OneOrMany::values));
    raw_paths.extend(autoload.classmap.values());
    raw_paths.extend(autoload.files.values());

    let mut paths = raw_paths
        .into_iter()
        .map(|path| {
            resolve_within(root, package_root, path, true)
                .map_err(|reason| CheckError::Composer(format!("invalid autoload path `{path}`: {reason}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    paths.dedup();
    Ok(paths)
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum InstalledPackages {
    ComposerOne(Vec<InstalledPackage>),
    ComposerTwo {
        #[serde(default)]
        packages: Vec<InstalledPackage>,
    },
}

impl InstalledPackages {
    fn packages(self) -> Vec<InstalledPackage> {
        match self {
            Self::ComposerOne(packages) | Self::ComposerTwo { packages } => packages,
        }
    }
}

#[derive(Debug, Deserialize)]
struct InstalledPackage {
    name: String,
    #[serde(rename = "install-path", default)]
    install_path: Option<String>,
    #[serde(default)]
    autoload: Autoload,
}

fn dependency_sources(
    root: &Path,
    vendor_dir: &Path,
    include_development_dependencies: bool,
) -> Result<Vec<PathBuf>, CheckError> {
    let installed_path = vendor_dir.join("composer").join("installed.json");
    let contents = match fs::read_to_string(&installed_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if lock_has_packages(root, include_development_dependencies)? {
                return Err(CheckError::Composer(format!(
                    "{} is missing after Riff installed locked packages",
                    installed_path.display()
                )));
            }
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(CheckError::Composer(format!(
                "could not read {}: {error}",
                installed_path.display()
            )));
        }
    };
    let installed: InstalledPackages = serde_json::from_str(&contents)?;
    let installed_parent = installed_path.parent().expect("installed.json always has a parent");
    let mut paths = Vec::new();

    for package in installed.packages() {
        let package_root = match package.install_path.as_deref() {
            Some(path) => resolve_within(root, installed_parent, path, false).map_err(|reason| {
                CheckError::Composer(format!("invalid install path for `{}`: {reason}", package.name))
            })?,
            None => resolve_within(root, vendor_dir, &package.name, false)
                .map_err(|reason| CheckError::Composer(format!("invalid package name `{}`: {reason}", package.name)))?,
        };
        paths.extend(autoload_sources(root, &package_root, &package.autoload)?);
    }

    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn lock_has_packages(root: &Path, include_development_dependencies: bool) -> Result<bool, CheckError> {
    let path = root.join("composer.lock");
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(CheckError::Composer(format!(
                "could not read {}: {error}",
                path.display()
            )));
        }
    };
    let lock: serde_json::Value = serde_json::from_str(&contents)?;
    let has_runtime = lock["packages"].as_array().is_some_and(|packages| !packages.is_empty());
    let has_development = include_development_dependencies
        && lock["packages-dev"]
            .as_array()
            .is_some_and(|packages| !packages.is_empty());
    Ok(has_runtime || has_development)
}

/// Resolve a Composer path without allowing an install or source scan to escape the checkout.
fn resolve_within(root: &Path, base: &Path, raw: &str, root_relative_slash: bool) -> Result<PathBuf, &'static str> {
    let raw = if root_relative_slash {
        raw.trim_start_matches(['/', '\\'])
    } else {
        raw
    };
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err("absolute paths are not allowed");
    }

    let mut resolved = base.to_path_buf();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => resolved.push(part),
            Component::ParentDir => {
                if !resolved.pop() || !resolved.starts_with(root) {
                    return Err("path escapes the repository checkout");
                }
            }
            Component::RootDir | Component::Prefix(_) => return Err("absolute paths are not allowed"),
        }
    }
    if !resolved.starts_with(root) {
        return Err("path escapes the repository checkout");
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "repository checkout cannot be resolved")?;
    let mut existing_ancestor = resolved.as_path();
    while !existing_ancestor.exists() {
        existing_ancestor = existing_ancestor
            .parent()
            .ok_or("Composer path has no existing ancestor")?;
    }
    let canonical_ancestor = existing_ancestor
        .canonicalize()
        .map_err(|_| "Composer path cannot be resolved")?;
    if !canonical_ancestor.starts_with(canonical_root) {
        return Err("path escapes the repository checkout through a symbolic link");
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    async fn install(
        session: &RiffSession,
        root: &Path,
        include_development_dependencies: bool,
        php_version: &str,
    ) -> Result<Installation, CheckError> {
        let pending = prepare_install(session, root, include_development_dependencies, php_version)?;
        let (request, plan) = pending.into_request_and_plan();
        let result = session
            .install_projects([request], riff_core::BatchOptions::default())
            .await
            .into_iter()
            .next()
            .expect("single project result");
        plan.finish(result)
    }

    fn write(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn discovers_only_root_autoload_and_honors_custom_vendor_directory() {
        let directory = TempDir::new().unwrap();
        write(
            &directory.path().join(COMPOSER_JSON),
            r#"{
                "autoload": {
                    "psr-0": {"Legacy_": "lib"},
                    "psr-4": {"App\\": ["src", "generated"]},
                    "classmap": ["classes", "src/Entry.php"],
                    "files": "functions.php"
                },
                "autoload-dev": {"psr-4": {"Tests\\": "tests"}},
                "config": {"vendor-dir": "build/dependencies"}
            }"#,
        );

        let manifest = read_manifest(directory.path()).unwrap();
        let sources = autoload_sources(directory.path(), directory.path(), &manifest.autoload).unwrap();
        assert_eq!(
            sources,
            ["classes", "functions.php", "generated", "lib", "src", "src/Entry.php"]
                .map(|path| directory.path().join(path))
        );
        assert_eq!(
            resolve_vendor_dir(
                directory.path(),
                manifest.config.and_then(|config| config.vendor_dir).as_deref()
            )
            .unwrap(),
            directory.path().join("build/dependencies")
        );
        assert!(!sources.iter().any(|path| path.ends_with("tests")));
    }

    #[test]
    fn reads_composer_one_and_two_installed_package_layouts() {
        for (document, expected) in [
            (
                r#"[{"name":"a/b","autoload":{"psr-4":{"A\\B\\":"src"},"files":["load.php"]}}]"#,
                vec!["vendor/a/b/load.php", "vendor/a/b/src"],
            ),
            (
                r#"{"packages":[{"name":"a/b","install-path":"../a/b","autoload":{"classmap":"classes"}}]}"#,
                vec!["vendor/a/b/classes"],
            ),
        ] {
            let directory = TempDir::new().unwrap();
            let composer_dir = directory.path().join("vendor/composer");
            fs::create_dir_all(&composer_dir).unwrap();
            write(&composer_dir.join("installed.json"), document);
            let sources = dependency_sources(directory.path(), &directory.path().join("vendor"), true).unwrap();
            assert_eq!(
                sources,
                expected
                    .into_iter()
                    .map(|path| directory.path().join(path))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn missing_installed_metadata_is_only_valid_without_selected_packages() {
        let directory = TempDir::new().unwrap();
        write(
            &directory.path().join("composer.lock"),
            r#"{"packages":[{"name":"a/b"}],"packages-dev":[]}"#,
        );
        assert!(dependency_sources(directory.path(), &directory.path().join("vendor"), false).is_err());

        write(
            &directory.path().join("composer.lock"),
            r#"{"packages":[],"packages-dev":[{"name":"a/b"}]}"#,
        );
        assert!(
            dependency_sources(directory.path(), &directory.path().join("vendor"), false)
                .unwrap()
                .is_empty()
        );
        assert!(dependency_sources(directory.path(), &directory.path().join("vendor"), true).is_err());
    }

    #[test]
    fn refuses_paths_that_escape_the_checkout() {
        let root = TempDir::new().unwrap();
        let path = root.path();
        assert!(resolve_within(path, &path.join("vendor/composer"), "../../../outside", false).is_err());
        assert!(resolve_within(path, path, "/src", false).is_err());
        assert_eq!(resolve_within(path, path, "/src", true).unwrap(), path.join("src"));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symbolic_links_that_escape_the_checkout() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), root.path().join("linked-source")).unwrap();
        assert!(resolve_within(root.path(), root.path(), "linked-source", false).is_err());
        assert!(resolve_within(root.path(), root.path(), "linked-source/not-created-yet", false).is_err());
    }

    #[test]
    fn embedded_riff_updates_lockless_projects_and_installs_locked_projects() {
        let directory = TempDir::new().unwrap();
        fs::create_dir(directory.path().join("src")).unwrap();
        write(
            &directory.path().join(COMPOSER_JSON),
            r#"{
                "name": "test/embedded-riff",
                "require": {"php": "^8.5", "ext-not-a-real-extension": "*"},
                "autoload": {"psr-4": {"Test\\": "src/"}},
                "config": {"vendor-dir": "build/dependencies"}
            }"#,
        );
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let session = RiffSession::new().unwrap();

        let first = runtime
            .block_on(install(&session, directory.path(), false, "8.5.9"))
            .unwrap();
        assert_eq!(first.project_sources(), &[directory.path().join("src")]);
        assert!(directory.path().join("composer.lock").is_file());
        assert!(
            directory
                .path()
                .join("build/dependencies/composer/installed.json")
                .is_file()
        );

        fs::remove_dir_all(directory.path().join("build/dependencies")).unwrap();
        let second = runtime
            .block_on(install(&session, directory.path(), false, "8.5.9"))
            .unwrap();
        assert_eq!(second.project_sources(), first.project_sources());
        assert!(
            directory
                .path()
                .join("build/dependencies/composer/installed.json")
                .is_file()
        );
    }

    #[test]
    fn embedded_riff_enforces_the_configured_php_version() {
        let directory = TempDir::new().unwrap();
        write(
            &directory.path().join(COMPOSER_JSON),
            r#"{
                "name": "test/embedded-riff-platform",
                "require": {"php": ">=9.0", "ext-not-a-real-extension": "*"}
            }"#,
        );
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let session = RiffSession::new().unwrap();

        let error = runtime
            .block_on(install(&session, directory.path(), false, "8.5.9"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("status 2"), "{error}");
    }
}
