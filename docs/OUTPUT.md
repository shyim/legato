# Output formats

Legato separates human diagnostics from machine-readable documents. This makes
commands such as `legato --format=json > report.json` safe without filtering
progress messages.

## Formats and streams

| Format | Stream | Intended consumer |
| --- | --- | --- |
| `console` | stderr | Interactive terminal |
| `markdown` | stdout | Release notes and pull-request comments |
| `github-actions` | stdout | GitHub workflow annotations |
| `json` | stdout | Scripts and custom integrations |
| `junit` | stdout | CI test-report importers |

`--format` may be repeated and documents are emitted in argument order. Progress
messages are shown only when `console` is among the requested formats.
Operational errors always use stderr.

## Console

The default report groups findings by affected source file and shows:

- compared commit IDs;
- source line and column when available;
- compatibility impact and modification type;
- the human-readable description;
- the stable rule identifier used by structured baselines; and
- totals plus baseline guidance.

Color defaults to terminal detection. `--color=always` forces ANSI styling and
`--color=never` disables it. Auto mode also honors `NO_COLOR`, `CLICOLOR`, and
`CLICOLOR_FORCE` through the output stream implementation.

## JSON

JSON contains one top-level `errors` array:

```json
{
  "errors": [
    {
      "description": "The parameter $value of Acme\\Api#change() changed from string to int",
      "path": "src/Api.php",
      "line": 7,
      "column": 28,
      "modificationType": "changed",
      "identifier": "method.parameter-type-changed",
      "sourcePath": "src/Api.php"
    }
  ]
}
```

`path`, `line`, and `column` describe the diagnostic location in the target
snapshot and may be `null`. `sourcePath` identifies the affected repository
file and can therefore point to the old path for a removed declaration. Stable
automation should key on `identifier`, not description text.

## GitHub Actions

This format emits one workflow command per finding. The rule identifier is the
annotation title; path, line, and column are included when available. Add the
command directly to a workflow step:

```sh
legato --format=github-actions
```

## JUnit

Each finding becomes a failed testcase. The rule identifier is the testcase
`classname`, the diagnostic position is its name, and the rendered finding is
the failure message. The historical upstream testsuite name is retained for
importer compatibility.

## Markdown

Markdown groups descriptions under Added, Changed, Removed, and Skipped
headings. It intentionally preserves the established finding descriptions for
release-note compatibility.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | No backwards-incompatible findings remain after baselines |
| `3` | One or more backwards-incompatible findings remain |
| `1` | Invalid input, configuration, Git, installation, parsing, or I/O failure |

Every currently registered rule is backwards-incompatible, including Added and
Skipped findings. See [`RULES.md`](../RULES.md) for the complete contract.
