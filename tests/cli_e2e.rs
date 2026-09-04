use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const OLD_SOURCE: &str = r#"<?php
namespace TestArtifact;

final class Api
{
    public function change(A $value): void {}
}
"#;

const NEW_SOURCE: &str = r#"<?php
namespace TestArtifact;

final class Api
{
    public function change(B $value): void {}
}
"#;

const OLD_BUILTIN_COLLECTION: &str = r#"<?php
namespace TestArtifact;

class Collection extends \ArrayObject {}
"#;

const NEW_BUILTIN_COLLECTION: &str = r#"<?php
namespace TestArtifact;

class Collection extends \ArrayObject
{
    public const STD_PROP_LIST = 2;
    public function count(): int { return parent::count(); }
}
"#;

const OLD_BUILTIN_FAILURE: &str = r#"<?php
namespace TestArtifact;

class Failure extends \Exception {}
"#;

const NEW_BUILTIN_FAILURE: &str = r#"<?php
namespace TestArtifact;

class Failure extends \Exception
{
    protected string $message = '';
}
"#;

#[test]
fn emits_and_answers_shell_completions() {
    let script = Command::new(env!("CARGO_BIN_EXE_legato"))
        .args(["completion", "zsh"])
        .output()
        .unwrap();
    assert!(script.status.success(), "{}", diagnostics(&script));
    assert!(script.stderr.is_empty(), "{}", diagnostics(&script));
    let script = String::from_utf8(script.stdout).unwrap();
    assert!(script.contains("__complete_word__"), "{script}");

    let answer = Command::new(env!("CARGO_BIN_EXE_legato"))
        .args(["__complete_word__", "--shell", "zsh", "--line", "legato --for"])
        .output()
        .unwrap();
    assert!(answer.status.success(), "{}", diagnostics(&answer));
    assert!(answer.stderr.is_empty(), "{}", diagnostics(&answer));
    assert!(String::from_utf8(answer.stdout).unwrap().contains("--format"));
}

#[test]
fn checks_two_real_revisions_and_keeps_json_machine_readable() {
    let repository = TempDir::new().unwrap();
    write_fixture(repository.path(), OLD_SOURCE);
    git(repository.path(), &["init", "--quiet"]);
    git(repository.path(), &["config", "user.email", "tests@example.com"]);
    git(repository.path(), &["config", "user.name", "Test Runner"]);
    git(repository.path(), &["config", "tag.gpgSign", "false"]);
    git(repository.path(), &["add", "."]);
    git(repository.path(), &["commit", "--quiet", "-m", "old API"]);
    git(repository.path(), &["tag", "1.0.0"]);

    fs::write(repository.path().join("src/Api.php"), NEW_SOURCE).unwrap();
    git(repository.path(), &["add", "src/Api.php"]);
    git(repository.path(), &["commit", "--quiet", "-m", "new API"]);

    let output = Command::new(env!("CARGO_BIN_EXE_legato"))
        .current_dir(repository.path())
        .args(["--format", "json", "--color=always"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{}", diagnostics(&output));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let document: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("stdout was not a standalone JSON document: {error}\n{stdout}"));
    let errors = document["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{stdout}");
    assert_eq!(
        errors[0]["description"],
        "The parameter $value of TestArtifact\\Api#change() changed from TestArtifact\\A to a non-contravariant TestArtifact\\B"
    );
    assert_eq!(errors[0]["modificationType"], "changed");
    assert_eq!(errors[0]["path"], "src/Api.php");
    assert_eq!(errors[0]["identifier"], "method.parameter-type-non-contravariant");
    assert_eq!(errors[0]["sourcePath"], "src/Api.php");

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.is_empty(), "{stderr}");
    assert!(!stdout.contains("Composer"), "{stdout}");

    let console_output = Command::new(env!("CARGO_BIN_EXE_legato"))
        .current_dir(repository.path())
        .output()
        .unwrap();
    assert_eq!(
        console_output.status.code(),
        Some(3),
        "{}",
        diagnostics(&console_output)
    );
    assert!(console_output.stdout.is_empty(), "{}", diagnostics(&console_output));
    let console_stderr = String::from_utf8(console_output.stderr).unwrap();
    assert!(
        console_stderr.contains("Detected last version: 1.0.0"),
        "{console_stderr}"
    );
    assert!(
        console_stderr.contains("No \"legato.toml\" found; using PHP 8.5.9 with all extensions"),
        "{console_stderr}"
    );
    assert!(console_stderr.contains("Comparing from "), "{console_stderr}");
    assert!(
        console_stderr.contains("Legato compatibility report"),
        "{console_stderr}"
    );
    assert!(
        console_stderr.contains("1 finding · 1 affected file"),
        "{console_stderr}"
    );
    assert!(console_stderr.contains("src/Api.php"), "{console_stderr}");
    assert!(console_stderr.contains("BREAKING  CHANGED"), "{console_stderr}");
    assert!(
        console_stderr.contains("rule: method.parameter-type-non-contravariant"),
        "{console_stderr}"
    );
    assert!(
        console_stderr.contains("Summary\n  1 breaking change"),
        "{console_stderr}"
    );
    assert!(console_stderr.contains("[[baseline.ignore]]"), "{console_stderr}");
    assert!(!console_stderr.contains("[BC]"), "{console_stderr}");
    assert!(!console_stderr.contains('\u{1b}'), "{console_stderr:?}");

    if let Some(upstream) = std::env::var_os("LEGATO_UPSTREAM") {
        let upstream_output = Command::new(upstream)
            .current_dir(repository.path())
            .arg("--format=json")
            .output()
            .unwrap();
        assert_eq!(
            upstream_output.status.code(),
            Some(3),
            "{}",
            diagnostics(&upstream_output)
        );
        let upstream_stdout = String::from_utf8(upstream_output.stdout).unwrap();
        let upstream_document: Value = serde_json::from_str(&upstream_stdout).unwrap_or_else(|error| {
            panic!("upstream stdout was not a standalone JSON document: {error}\n{upstream_stdout}")
        });
        assert_eq!(upstream_document["errors"].as_array().unwrap().len(), errors.len());
        assert_eq!(upstream_document["errors"][0]["description"], errors[0]["description"]);
        assert_eq!(
            upstream_document["errors"][0]["modificationType"],
            errors[0]["modificationType"]
        );
    }
}

#[test]
fn matches_upstream_for_members_inherited_from_php_builtins() {
    let Some(upstream) = std::env::var_os("LEGATO_UPSTREAM") else {
        return;
    };
    let repository = TempDir::new().unwrap();
    write_builtin_fixture(repository.path(), OLD_BUILTIN_COLLECTION, OLD_BUILTIN_FAILURE);
    git(repository.path(), &["init", "--quiet"]);
    git(repository.path(), &["config", "user.email", "tests@example.com"]);
    git(repository.path(), &["config", "user.name", "Test Runner"]);
    git(repository.path(), &["config", "tag.gpgSign", "false"]);
    git(repository.path(), &["add", "."]);
    git(repository.path(), &["commit", "--quiet", "-m", "old built-in API"]);
    git(repository.path(), &["tag", "1.0.0"]);
    fs::write(repository.path().join("src/Collection.php"), NEW_BUILTIN_COLLECTION).unwrap();
    fs::write(repository.path().join("src/Failure.php"), NEW_BUILTIN_FAILURE).unwrap();
    git(repository.path(), &["add", "src/Collection.php", "src/Failure.php"]);
    git(repository.path(), &["commit", "--quiet", "-m", "new built-in API"]);

    let rust_output = Command::new(env!("CARGO_BIN_EXE_legato"))
        .current_dir(repository.path())
        .arg("--format=json")
        .output()
        .unwrap();
    let upstream_output = Command::new(upstream)
        .current_dir(repository.path())
        .arg("--format=json")
        .output()
        .unwrap();

    assert_eq!(rust_output.status.code(), Some(3), "{}", diagnostics(&rust_output));
    assert_eq!(
        upstream_output.status.code(),
        Some(3),
        "{}",
        diagnostics(&upstream_output)
    );
    let mut rust_errors = error_signatures(&rust_output);
    let mut upstream_errors = error_signatures(&upstream_output);
    assert_eq!(rust_errors.len(), 3, "{}", diagnostics(&rust_output));
    rust_errors.sort();
    upstream_errors.sort();
    assert_eq!(rust_errors, upstream_errors);
}

#[test]
fn applies_toml_exclusions_and_structured_ignores() {
    let repository = TempDir::new().unwrap();
    fs::create_dir_all(repository.path().join("src/Internal")).unwrap();
    fs::write(
        repository.path().join("composer.json"),
        r#"{
    "name": "test/legato-config-fixture",
    "autoload": {"psr-4": {"TestArtifact\\": "src/"}}
}
"#,
    )
    .unwrap();
    fs::write(
        repository.path().join("legato.toml"),
        r#"[platform]
php = "8.5.9"
extensions = "all"

[paths]
exclude = ["src/Internal/**"]

[baseline]

[[baseline.ignore]]
identifier = "method.removed"
path = "src/Ignored.php"
"#,
    )
    .unwrap();
    for (path, namespace, class) in [
        ("src/Api.php", "TestArtifact", "Api"),
        ("src/Ignored.php", "TestArtifact", "Ignored"),
        ("src/Internal/Hidden.php", "TestArtifact\\Internal", "Hidden"),
    ] {
        fs::write(
            repository.path().join(path),
            format!("<?php namespace {namespace}; class {class} {{ public function removed(): void {{}} }}\n"),
        )
        .unwrap();
    }

    git(repository.path(), &["init", "--quiet"]);
    git(repository.path(), &["config", "user.email", "tests@example.com"]);
    git(repository.path(), &["config", "user.name", "Test Runner"]);
    git(repository.path(), &["config", "tag.gpgSign", "false"]);
    git(repository.path(), &["add", "."]);
    git(repository.path(), &["commit", "--quiet", "-m", "old API"]);
    git(repository.path(), &["tag", "1.0.0"]);

    for (path, namespace, class) in [
        ("src/Api.php", "TestArtifact", "Api"),
        ("src/Ignored.php", "TestArtifact", "Ignored"),
        ("src/Internal/Hidden.php", "TestArtifact\\Internal", "Hidden"),
    ] {
        fs::write(
            repository.path().join(path),
            format!("<?php namespace {namespace}; class {class} {{}}\n"),
        )
        .unwrap();
    }
    git(repository.path(), &["add", "src"]);
    git(repository.path(), &["commit", "--quiet", "-m", "new API"]);

    let output = Command::new(env!("CARGO_BIN_EXE_legato"))
        .current_dir(repository.path())
        .arg("--format=json")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3), "{}", diagnostics(&output));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    let errors = document["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{}", diagnostics(&output));
    assert_eq!(errors[0]["path"], "src/Api.php");
    assert_eq!(errors[0]["identifier"], "method.removed");
    assert_eq!(errors[0]["sourcePath"], "src/Api.php");
    assert!(
        errors[0]["description"].as_str().unwrap().contains("TestArtifact\\Api"),
        "{}",
        diagnostics(&output)
    );
}

fn write_fixture(root: &Path, source: &str) {
    fs::create_dir(root.join("src")).unwrap();
    fs::write(
        root.join("composer.json"),
        r#"{
    "name": "test/legato-fixture",
    "autoload": {"psr-4": {"TestArtifact\\": "src/"}}
}
"#,
    )
    .unwrap();
    fs::write(root.join("src/A.php"), "<?php namespace TestArtifact; interface A {}\n").unwrap();
    fs::write(root.join("src/B.php"), "<?php namespace TestArtifact; interface B {}\n").unwrap();
    fs::write(root.join("src/Api.php"), source).unwrap();
}

fn write_builtin_fixture(root: &Path, collection: &str, failure: &str) {
    fs::create_dir(root.join("src")).unwrap();
    fs::write(
        root.join("composer.json"),
        r#"{
    "name": "test/legato-fixture",
    "autoload": {"psr-4": {"TestArtifact\\": "src/"}}
}
"#,
    )
    .unwrap();
    fs::write(root.join("src/Collection.php"), collection).unwrap();
    fs::write(root.join("src/Failure.php"), failure).unwrap();
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", diagnostics(&output));
}

fn diagnostics(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn error_signatures(output: &Output) -> Vec<(String, String)> {
    let document: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid JSON output: {error}\n{}", diagnostics(output)));
    document["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| {
            (
                error["description"].as_str().unwrap().to_owned(),
                error["modificationType"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}
