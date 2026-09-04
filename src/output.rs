use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;

use serde::Serialize;

use crate::{Change, CheckError, CheckReport, ModificationType};

const JUNIT_SCHEMA: &str = "https://raw.githubusercontent.com/junit-team/junit5/732a5400f80c8f446daa8b43eaa4b41b3da929be/platform-tests/src/test/resources/jenkins-junit.xsd";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum OutputFormat {
    Console,
    Markdown,
    GithubActions,
    Json,
    Junit,
}

impl OutputFormat {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 5] = [
        Self::Console,
        Self::Markdown,
        Self::GithubActions,
        Self::Json,
        Self::Junit,
    ];

    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Console => "console",
            Self::Markdown => "markdown",
            Self::GithubActions => "github-actions",
            Self::Json => "json",
            Self::Junit => "junit",
        }
    }

    #[must_use]
    pub(crate) const fn destination(self) -> OutputDestination {
        match self {
            Self::Console => OutputDestination::Stderr,
            Self::Markdown | Self::GithubActions | Self::Json | Self::Junit => OutputDestination::Stdout,
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "console" => Ok(Self::Console),
            "markdown" => Ok(Self::Markdown),
            "github-actions" => Ok(Self::GithubActions),
            "json" => Ok(Self::Json),
            "junit" => Ok(Self::Junit),
            _ => Err(format!("Unsupported output format `{value}`.")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputDestination {
    Stdout,
    Stderr,
}

pub(crate) fn write(
    formats: &[OutputFormat],
    report: &CheckReport,
    target_checkout: &Path,
    ansi: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CheckError> {
    for format in formats {
        let rendered = render(*format, report, target_checkout, ansi)?;
        match format.destination() {
            OutputDestination::Stdout => stdout.write_all(rendered.as_bytes())?,
            OutputDestination::Stderr => stderr.write_all(rendered.as_bytes())?,
        }
    }
    Ok(())
}

pub(crate) fn render(
    format: OutputFormat,
    report: &CheckReport,
    target_checkout: &Path,
    ansi: bool,
) -> Result<String, CheckError> {
    match format {
        OutputFormat::Console => Ok(render_console(report, target_checkout, ansi)),
        OutputFormat::Markdown => Ok(render_markdown(&report.changes)),
        OutputFormat::GithubActions => Ok(render_github_actions(&report.changes, target_checkout)),
        OutputFormat::Json => render_json(&report.changes, target_checkout),
        OutputFormat::Junit => Ok(render_junit(&report.changes, target_checkout)),
    }
}

fn render_console(report: &CheckReport, target_checkout: &Path, ansi: bool) -> String {
    let mut output = String::new();
    let breaking = report.changes.iter().filter(|change| change.is_breaking()).count();
    let informational = report.changes.len() - breaking;
    let from = short_revision(&report.from_revision);
    let to = short_revision(&report.to_revision);

    writeln!(output, "{}", style("Legato compatibility report", "1;36", ansi)).unwrap();
    writeln!(
        output,
        "  {} {} {}",
        style(&from, "1", ansi),
        style("→", "2", ansi),
        style(&to, "1", ansi)
    )
    .unwrap();

    if report.changes.is_empty() {
        writeln!(
            output,
            "\n{} No backwards-incompatible changes found.",
            style("✓", "1;32", ansi)
        )
        .unwrap();
        return output;
    }

    let mut groups: BTreeMap<String, Vec<&Change>> = BTreeMap::new();
    let mut ungrouped = Vec::new();
    for change in &report.changes {
        let path = change
            .source_path()
            .map(|path| relative_path(path, target_checkout))
            .filter(|path| !path.is_empty());
        if let Some(path) = path {
            groups.entry(path).or_default().push(change);
        } else {
            ungrouped.push(change);
        }
    }

    let file_count = groups.len();
    let finding_label = plural(report.changes.len(), "finding", "findings");
    if file_count == 0 {
        writeln!(output, "  {} {finding_label}", report.changes.len()).unwrap();
    } else {
        let file_label = plural(file_count, "file", "files");
        writeln!(
            output,
            "  {} {finding_label} · {file_count} affected {file_label}",
            report.changes.len()
        )
        .unwrap();
    }

    for (path, changes) in groups {
        render_console_group(&mut output, &path, &changes, target_checkout, ansi);
    }
    if !ungrouped.is_empty() {
        render_console_group(&mut output, "Other findings", &ungrouped, target_checkout, ansi);
    }

    writeln!(output, "\n{}", style("Summary", "1", ansi)).unwrap();
    let breaking_summary = count_phrase(breaking, "breaking change", "breaking changes");
    let informational_summary = count_phrase(informational, "informational finding", "informational findings");
    writeln!(
        output,
        "  {} · {}",
        style(&breaking_summary, if breaking == 0 { "2" } else { "1;31" }, ansi),
        style(&informational_summary, "36", ansi)
    )
    .unwrap();
    let modification_counts = [
        (ModificationType::Added, "added"),
        (ModificationType::Changed, "changed"),
        (ModificationType::Removed, "removed"),
        (ModificationType::Skipped, "skipped"),
    ]
    .into_iter()
    .filter_map(|(kind, label)| {
        let count = report
            .changes
            .iter()
            .filter(|change| change.modification_type() == kind)
            .count();
        (count != 0).then(|| format!("{count} {label}"))
    })
    .collect::<Vec<_>>()
    .join(" · ");
    writeln!(output, "  {modification_counts}").unwrap();

    if breaking != 0 {
        writeln!(
            output,
            "\n{} Suppress intentional findings with {} in {}.",
            style("Hint:", "1;33", ansi),
            style("[[baseline.ignore]]", "1", ansi),
            style("legato.toml", "1", ansi)
        )
        .unwrap();
    }
    output
}

fn render_console_group(output: &mut String, heading: &str, changes: &[&Change], target_checkout: &Path, ansi: bool) {
    writeln!(output, "\n{}", style(heading, "1;36", ansi)).unwrap();
    let locations = changes
        .iter()
        .map(|change| console_location(change, Some(heading), target_checkout))
        .collect::<Vec<_>>();
    let location_width = locations
        .iter()
        .map(|location| location.chars().count())
        .max()
        .unwrap_or(1);
    let description_offset = location_width + 23;

    for (change, location) in changes.iter().copied().zip(locations) {
        let severity_label = if change.is_breaking() { "BREAKING" } else { "INFO" };
        let severity = style(
            &format!("{severity_label:<8}"),
            if change.is_breaking() { "1;31" } else { "36" },
            ansi,
        );
        let modification = style(
            &format!("{:<7}", change.modification_type().label()),
            match change.modification_type() {
                ModificationType::Added => "32",
                ModificationType::Changed => "33",
                ModificationType::Removed => "31",
                ModificationType::Skipped => "35",
            },
            ansi,
        );
        writeln!(
            output,
            "  {location:>location_width$}  {severity}  {modification}  {}",
            change.description
        )
        .unwrap();
        writeln!(
            output,
            "{:description_offset$}{}",
            "",
            style(&format!("rule: {}", change.identifier()), "2", ansi)
        )
        .unwrap();
    }
}

fn short_revision(revision: &str) -> String {
    revision.chars().take(12).collect()
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn count_phrase(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", self::plural(count, singular, plural))
}

fn console_location(change: &Change, source_path: Option<&str>, target_checkout: &Path) -> String {
    let Some(location) = &change.location else {
        return "—".to_owned();
    };
    let location_path = relative_path(&location.path, target_checkout);
    if location_path.is_empty() || source_path == Some(location_path.as_str()) {
        format!("{}:{}", location.line, location.column)
    } else {
        format!("{location_path}:{}:{}", location.line, location.column)
    }
}

fn style(value: &str, code: &str, ansi: bool) -> String {
    if ansi {
        format!("\x1b[{code}m{value}\x1b[0m")
    } else {
        value.to_owned()
    }
}

fn render_markdown(changes: &[Change]) -> String {
    let mut output = String::new();
    let groups = [
        ("Added", ModificationType::Added),
        ("Changed", ModificationType::Changed),
        ("Removed", ModificationType::Removed),
        ("Skipped", ModificationType::Skipped),
    ];

    for (index, (heading, modification_type)) in groups.into_iter().enumerate() {
        if index != 0 {
            output.push('\n');
        }
        output.push_str("# ");
        output.push_str(heading);
        output.push('\n');
        for change in changes
            .iter()
            .filter(|change| change.modification_type() == modification_type)
        {
            let mut rendered = php_trim(&change.to_string()).to_owned();
            for label in ["ADDED: ", "CHANGED: ", "REMOVED: ", "SKIPPED: "] {
                rendered = rendered.replace(label, "");
            }
            output.push_str(" - ");
            output.push_str(&rendered);
            output.push('\n');
        }
    }

    // Symfony's writeln() adds a line ending after a string that already ends in one.
    output.push('\n');
    output
}

fn render_github_actions(changes: &[Change], target_checkout: &Path) -> String {
    let mut output = String::new();
    for change in changes {
        let message = escape_github_data(&change.description);
        let title = escape_github_property(change.identifier());
        match &change.location {
            None => {
                output.push_str("::error title=");
                output.push_str(&title);
                output.push_str("::");
                output.push_str(&message);
                output.push('\n');
            }
            Some(location) => {
                let filename = relative_path(&location.path, target_checkout);
                if filename.is_empty() {
                    output.push_str("::error title=");
                    output.push_str(&title);
                    output.push_str("::");
                    output.push_str(&message);
                    output.push('\n');
                    continue;
                }
                output.push_str("::error file=");
                output.push_str(&escape_github_property(&filename));
                output.push_str(",line=");
                output.push_str(&location.line.to_string());
                output.push_str(",col=");
                output.push_str(&location.column.to_string());
                output.push_str(",title=");
                output.push_str(&title);
                output.push_str("::");
                output.push_str(&message);
                output.push('\n');
            }
        }
    }
    output
}

fn render_json(changes: &[Change], target_checkout: &Path) -> Result<String, CheckError> {
    let errors = changes
        .iter()
        .map(|change| {
            let (path, line, column) = match &change.location {
                Some(location) => (
                    Some(relative_path(&location.path, target_checkout)),
                    Some(location.line),
                    Some(location.column),
                ),
                None => (None, None, None),
            };
            JsonChange {
                description: &change.description,
                path,
                line,
                column,
                modification_type: change.modification_type().as_str(),
                identifier: change.identifier(),
                source_path: change.source_path().map(|path| relative_path(path, target_checkout)),
            }
        })
        .collect();
    let mut output = serde_json::to_string(&JsonOutput { errors })?;
    output.push('\n');
    Ok(output)
}

fn render_junit(changes: &[Change], target_checkout: &Path) -> String {
    let mut output = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"roave/backward-compatibility-check\" tests=\"{count}\" failures=\"{count}\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"{JUNIT_SCHEMA}\">\n",
        count = changes.len(),
    );

    for change in changes {
        let (filename, line, column) = match &change.location {
            Some(location) => (
                relative_path(&location.path, target_checkout),
                location.line.to_string(),
                location.column.to_string(),
            ),
            None => (String::new(), String::new(), String::new()),
        };
        let testcase_name = format!("{filename}:{line}:{column}");
        let testcase_name = escape_xml_attribute(&escape_xml_attribute(&testcase_name));
        let classname = escape_xml_attribute(change.identifier());
        let message = escape_xml_attribute(php_trim(&change.to_string()));
        output.push_str("  <testcase name=\"");
        output.push_str(&testcase_name);
        output.push_str("\" classname=\"");
        output.push_str(&classname);
        output.push_str("\"><failure type=\"error\" message=\"");
        output.push_str(&message);
        output.push_str("\"/></testcase>\n");
    }

    output.push_str("</testsuite>\n");
    output
}

#[derive(Serialize)]
struct JsonOutput<'a> {
    errors: Vec<JsonChange<'a>>,
}

#[derive(Serialize)]
struct JsonChange<'a> {
    description: &'a str,
    path: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    #[serde(rename = "modificationType")]
    modification_type: &'static str,
    identifier: &'static str,
    #[serde(rename = "sourcePath")]
    source_path: Option<String>,
}

fn relative_path(path: &Path, target_checkout: &Path) -> String {
    let path = path.to_string_lossy();
    let mut base = target_checkout.to_string_lossy().into_owned();
    base.push('/');
    path.replace(&base, "")
}

fn escape_github_data(value: &str) -> String {
    escape_github(value, false)
}

fn escape_github_property(value: &str) -> String {
    escape_github(value, true)
}

fn escape_github(value: &str, property: bool) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '%' => output.push_str("%25"),
            '\r' => output.push_str("%0D"),
            '\n' => output.push_str("%0A"),
            ':' if property => output.push_str("%3A"),
            ',' if property => output.push_str("%2C"),
            _ => output.push(character),
        }
    }
    output
}

fn escape_xml_attribute(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '"' => output.push_str("&quot;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(character),
        }
    }
    output
}

fn php_trim(value: &str) -> &str {
    value.trim_matches(|character| {
        matches!(
            character,
            ' ' | '\t' | '\n' | '\r' | '\0' | '\u{000B}' | '\u{000C}' | '\u{00A0}' | '\u{FEFF}'
        )
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::{Rule, SourceLocation};

    fn changes(base: &Path) -> Vec<Change> {
        vec![
            Change::new(Rule::CLASS_REMOVED, "foo"),
            Change::new(Rule::INTERFACE_METHOD_ADDED, "bar"),
            Change::new(Rule::CLASS_BECAME_FINAL, "baz").at(SourceLocation {
                path: PathBuf::from("baz-file.php"),
                line: 1,
                column: 0,
            }),
            Change::new(
                Rule::PROPERTY_DEFAULT_VALUE_COMPARISON_UNSUPPORTED,
                "file-in-checked-out-dir",
            )
            .at(SourceLocation {
                path: base.join("subpath/file-in-checked-out-dir.php"),
                line: 10,
                column: 20,
            }),
        ]
    }

    fn report(changes: Vec<Change>) -> CheckReport {
        CheckReport {
            from_revision: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            to_revision: "fedcba9876543210fedcba9876543210fedcba98".to_owned(),
            changes,
        }
    }

    #[test]
    fn documented_json_example_is_valid_and_uses_stable_rule_metadata() {
        let document = include_str!("../docs/OUTPUT.md");
        let json = document
            .split_once("```json\n")
            .and_then(|(_, remainder)| remainder.split_once("\n```"))
            .map(|(block, _)| block)
            .expect("output documentation contains a JSON example");
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        let finding = &parsed["errors"][0];

        assert_eq!(finding["identifier"], "method.parameter-type-changed");
        assert_eq!(finding["modificationType"], "changed");
        assert_eq!(finding["sourcePath"], "src/Api.php");
    }

    #[test]
    fn console_groups_findings_and_explains_the_result() {
        let base = Path::new("/tmp/checkout");
        let output = render_console(&report(changes(base)), base, false);

        assert!(output.starts_with("Legato compatibility report\n  0123456789ab → fedcba987654\n"));
        assert!(output.contains("  4 findings · 2 affected files\n"));
        assert!(output.contains("\nbaz-file.php\n  1:0  BREAKING  CHANGED  baz\n"));
        assert!(output.contains("rule: class.became-final"));
        assert!(output.contains("\nsubpath/file-in-checked-out-dir.php\n"));
        assert!(output.contains("\nOther findings\n"));
        assert!(output.contains("—  BREAKING  REMOVED  foo\n"));
        assert!(output.contains("\nSummary\n  4 breaking changes · 0 informational findings\n"));
        assert!(output.contains("  1 added · 1 changed · 1 removed · 1 skipped\n"));
        assert!(output.ends_with("\nHint: Suppress intentional findings with [[baseline.ignore]] in legato.toml.\n"));
        assert!(!output.contains("[BC]"));
        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn console_renders_a_compact_success_result() {
        let output = render_console(&report(Vec::new()), Path::new("/tmp/checkout"), false);
        assert_eq!(
            output,
            "Legato compatibility report\n  0123456789ab → fedcba987654\n\n✓ No backwards-incompatible changes found.\n"
        );
    }

    #[test]
    fn console_applies_ansi_styling_without_affecting_alignment() {
        let output = render_console(
            &report(vec![
                Change::new(Rule::CLASS_REMOVED, "Class A has been deleted").in_path("src/A.php"),
            ]),
            Path::new("/tmp/checkout"),
            true,
        );
        assert!(output.contains("\x1b[1;36mLegato compatibility report\x1b[0m"));
        assert!(output.contains("\x1b[1;31mBREAKING\x1b[0m"));
        assert!(output.contains("\x1b[31mREMOVED\x1b[0m"));
        assert!(output.contains("rule: class.removed"));
    }

    #[test]
    fn markdown_groups_changes_in_fixed_order() {
        let changes = vec![
            Change::new(Rule::CLASS_BECAME_FINAL, "Something changed"),
            Change::new(Rule::CLASS_REMOVED, "Something removed"),
            Change::new(Rule::INTERFACE_METHOD_ADDED, "Something added"),
            Change::new(Rule::CONSTANT_VALUE_COMPARISON_UNSUPPORTED, "A failure happened"),
        ];
        assert_eq!(
            render_markdown(&changes),
            "# Added\n - [BC] Something added\n\n# Changed\n - [BC] Something changed\n\n# Removed\n - [BC] Something removed\n\n# Skipped\n - [BC] A failure happened\n\n"
        );
    }

    #[test]
    fn markdown_preserves_the_upstream_global_label_replacement() {
        let output = render_markdown(&[Change::new(
            Rule::CLASS_BECAME_FINAL,
            "A CHANGED: token remains surprising",
        )]);
        assert!(output.contains(" - [BC] A token remains surprising\n"));
    }

    #[test]
    fn github_actions_adds_identifier_titles_and_escapes_values() {
        let base = Path::new("/tmp/checkout");
        let mut input = changes(base);
        input.push(
            Change::new(Rule::PROPERTY_TYPE_CHANGED, "100%\r\nfailed").at(SourceLocation {
                path: base.join("dir:name,file.php"),
                line: 6,
                column: 15,
            }),
        );
        assert_eq!(
            render_github_actions(&input, base),
            "::error title=class.removed::foo\n::error title=interface.method-added::bar\n::error file=baz-file.php,line=1,col=0,title=class.became-final::baz\n::error file=subpath/file-in-checked-out-dir.php,line=10,col=20,title=property.default-value-comparison-unsupported::file-in-checked-out-dir\n::error file=dir%3Aname%2Cfile.php,line=6,col=15,title=property.type-changed::100%25%0D%0Afailed\n"
        );
    }

    #[test]
    fn json_adds_finding_metadata_after_the_upstream_fields() {
        let base = Path::new("/tmp/checkout");
        let output = render_json(&changes(base), base).unwrap();
        assert_eq!(
            output,
            concat!(
                r#"{"errors":[{"description":"foo","path":null,"line":null,"column":null,"modificationType":"removed","identifier":"class.removed","sourcePath":null},{"description":"bar","path":null,"line":null,"column":null,"modificationType":"added","identifier":"interface.method-added","sourcePath":null},{"description":"baz","path":"baz-file.php","line":1,"column":0,"modificationType":"changed","identifier":"class.became-final","sourcePath":"baz-file.php"},{"description":"file-in-checked-out-dir","path":"subpath/file-in-checked-out-dir.php","line":10,"column":20,"modificationType":"skipped","identifier":"property.default-value-comparison-unsupported","sourcePath":"subpath/file-in-checked-out-dir.php"}]}"#,
                "\n"
            )
        );
    }

    #[test]
    fn json_exposes_a_source_path_without_changing_the_diagnostic_location() {
        let change = Change::new(Rule::CLASS_REMOVED, "removed").in_path("src/Removed.php");
        let output = render_json(&[change], Path::new("/tmp/checkout")).unwrap();
        assert_eq!(
            output,
            "{\"errors\":[{\"description\":\"removed\",\"path\":null,\"line\":null,\"column\":null,\"modificationType\":\"removed\",\"identifier\":\"class.removed\",\"sourcePath\":\"src/Removed.php\"}]}\n"
        );
    }

    #[test]
    fn junit_adds_identifier_classnames_to_the_upstream_shape() {
        let base = Path::new("/tmp/checkout");
        let expected = format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
                "<testsuite name=\"roave/backward-compatibility-check\" tests=\"4\" failures=\"4\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"{}\">\n",
                "  <testcase name=\"::\" classname=\"class.removed\"><failure type=\"error\" message=\"[BC] REMOVED: foo\"/></testcase>\n",
                "  <testcase name=\"::\" classname=\"interface.method-added\"><failure type=\"error\" message=\"[BC] ADDED: bar\"/></testcase>\n",
                "  <testcase name=\"baz-file.php:1:0\" classname=\"class.became-final\"><failure type=\"error\" message=\"[BC] CHANGED: baz\"/></testcase>\n",
                "  <testcase name=\"subpath/file-in-checked-out-dir.php:10:20\" classname=\"property.default-value-comparison-unsupported\"><failure type=\"error\" message=\"[BC] SKIPPED: file-in-checked-out-dir\"/></testcase>\n",
                "</testsuite>\n",
            ),
            JUNIT_SCHEMA,
        );
        assert_eq!(render_junit(&changes(base), base), expected,);
    }

    #[test]
    fn junit_double_escapes_the_testcase_name_like_the_upstream_formatter() {
        let change = Change::new(Rule::PROPERTY_TYPE_CHANGED, "A & \"B\" < C > D").at(SourceLocation {
            path: PathBuf::from("a&\"b<c>d.php"),
            line: 1,
            column: 2,
        });
        let output = render_junit(&[change], Path::new("/tmp/checkout"));
        assert!(output.contains("name=\"a&amp;amp;&amp;quot;b&amp;lt;c&amp;gt;d.php:1:2\""));
        assert!(output.contains("classname=\"property.type-changed\""));
        assert!(output.contains("message=\"[BC] CHANGED: A &amp; &quot;B&quot; &lt; C &gt; D\""));
    }

    #[test]
    fn repeated_formats_keep_order_and_use_the_expected_streams() {
        let formats = [OutputFormat::Json, OutputFormat::Console, OutputFormat::Markdown];
        let report = report(vec![Change::new(Rule::CLASS_REMOVED, "foo")]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        write(
            &formats,
            &report,
            Path::new("/tmp/checkout"),
            false,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        let stdout = String::from_utf8(stdout).unwrap();
        assert!(stdout.starts_with("{\"errors\":"));
        assert!(stdout.ends_with("# Skipped\n\n"));
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("Legato compatibility report"));
        assert!(stderr.contains("BREAKING  REMOVED  foo"));
    }

    #[test]
    fn parses_all_cli_format_names() {
        for format in OutputFormat::ALL {
            assert_eq!(format.as_str().parse(), Ok(format));
        }
        assert!("yaml".parse::<OutputFormat>().is_err());
    }
}
