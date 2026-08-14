//! Integration tests that run the `data-dict` binary end to end.

use std::path::PathBuf;
use std::process::Command;

/// Running `data-dict` with no arguments lists every subcommand.
///
/// When this snapshot changes (i.e. the set of commands changes), update the
/// command listing under "### Usage" in the repo-root README.md to match.
#[test]
fn no_args_lists_all_subcommands() {
    let output = Command::new(env!("CARGO_BIN_EXE_data-dict"))
        .output()
        .expect("failed to run data-dict");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout is not valid UTF-8");
    insta::assert_snapshot!(stdout);
}

/// A fixture that fails schema validation with two errors (S07, S08) and a warning (S09),
/// in that emission order. Validating its data skips the data comparison (the
/// dictionary has errors), so no source is ever read.
fn multi_error_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-error-with-warning.yaml")
}

/// The default (text) output renders every diagnostic — both errors and the
/// warning — to stderr, in emission order.
#[test]
fn multiple_diagnostics_text_output() {
    let fixture = multi_error_fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_data-dict"))
        .args(["validate-data"])
        .arg(&fixture)
        .output()
        .expect("failed to run data-dict");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr is not valid UTF-8");
    insta::assert_snapshot!(sanitize(&stderr, &fixture.display().to_string()));
}

/// The `--json` output carries the same diagnostics as a structured array,
/// preserving severity, code, and emission order.
#[test]
fn multiple_diagnostics_json_output() {
    let fixture = multi_error_fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_data-dict"))
        .args(["validate-data"])
        .arg(&fixture)
        .arg("--json")
        .output()
        .expect("failed to run data-dict");
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout is not valid UTF-8");
    // Re-serialize so the snapshot is pretty-printed and key order is stable.
    let mut value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    // The run names a fixture in a temp directory, the clock moves, and the
    // version changes on release; the snapshot is about the findings.
    value["run"]["dictionary"] = "<fixture>".into();
    value["run"]["generated_at"] = "<timestamp>".into();
    value["run"]["tool"]["version"] = "<version>".into();
    insta::assert_snapshot!(serde_json::to_string_pretty(&value).unwrap());
}

/// A spec-level run reports through the same document, with no steps: its
/// checks read the document as a whole rather than any declared target.
#[test]
fn validate_spec_json_report() {
    let fixture = multi_error_fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_data-dict"))
        .args(["validate-spec"])
        .arg(&fixture)
        .arg("--json")
        .output()
        .expect("failed to run data-dict");
    assert!(!output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is valid JSON");
    assert_eq!(report["$version"], data_dict::REPORT_VERSION);
    assert_eq!(report["status"], "error");
    assert_eq!(report["steps"], serde_json::json!([]));
    assert!(report["problems"].as_array().is_some_and(|p| !p.is_empty()));
}

/// A report says which level ran, which is not what its findings reveal: a
/// data-level run stopped by a spec error still reports the data level.
#[test]
fn the_report_says_what_the_run_was() {
    let fixture = multi_error_fixture();
    for (command, level) in [
        ("validate-spec", "spec"),
        ("validate-meta", "meta"),
        ("validate-data", "data"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_data-dict"))
            .args([command])
            .arg(&fixture)
            .arg("--json")
            .output()
            .expect("failed to run data-dict");
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("stdout is valid JSON");
        let run = &report["run"];
        assert_eq!(run["level"], level, "{command}");
        assert_eq!(run["dictionary"], fixture.display().to_string());
        assert_eq!(run["tool"]["name"], "data-dict");
        assert_eq!(run["tool"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(
            run["table"].is_null(),
            "a whole-dictionary run names no table"
        );
        let at = run["generated_at"].as_str().expect("a timestamp");
        assert!(at.ends_with('Z') && at.len() == 20, "{at}");
    }
}

/// A run over one table says so, which its steps can't: a one-table dictionary
/// and a single-table run over a larger one list the same steps.
#[test]
fn the_report_names_a_single_table_run() {
    let dir = temp_dir("run-table");
    write_render_dict(&dir, "One row per pup.");
    write_parquet(&dir.join("pups.parquet"), "pup_count", &[0, 1, 1, 2]);

    let output = run_in(&dir, &["validate-data", "--table", "pups", "--json"]);
    assert!(output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is valid JSON");
    assert_eq!(report["run"]["table"], "pups");
}

/// A failure that stopped the run before any check could be applied is not a
/// finding about the dictionary, so it is reported as a plain error on stderr
/// and no report is written (see `site/report.md`).
#[test]
fn a_preflight_failure_writes_no_report() {
    let dir = temp_dir("preflight");
    let output = Command::new(env!("CARGO_BIN_EXE_data-dict"))
        .args(["validate-data"])
        .arg(dir.join("absent.yaml"))
        .arg("--json")
        .output()
        .expect("failed to run data-dict");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "no report should be written");
    // The wording is the operating system's, so only its presence is asserted.
    let stderr = String::from_utf8(output.stderr).expect("stderr is not valid UTF-8");
    assert!(!stderr.trim().is_empty(), "the failure should be reported");
}

/// Rewrite the fixture's absolute path to a stable placeholder so the rendered
/// diagnostic can be snapshotted. The CLI already renders plain (no colour) when
/// its stderr is a pipe, as it is under the test harness, so there is no
/// terminal styling to strip.
fn sanitize(s: &str, fixture_path: &str) -> String {
    s.replace(fixture_path, "<fixture>")
}

// --- shared parquet fixtures -------------------------------------------

/// A unique temp directory for one test's fixtures.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("data-dict-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a one-column INT64 parquet file at `path`.
fn write_parquet(path: &std::path::Path, column: &str, values: &[i64]) {
    use parquet::data_type::Int64Type;
    use parquet::file::properties::WriterProperties;
    use parquet::file::writer::SerializedFileWriter;
    use parquet::schema::parser::parse_message_type;
    use std::sync::Arc;

    let message = format!("message schema {{ REQUIRED INT64 {column}; }}");
    let schema = Arc::new(parse_message_type(&message).unwrap());
    let file = std::fs::File::create(path).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::new())).unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    let mut col = row_group.next_column().unwrap().unwrap();
    col.typed::<Int64Type>()
        .write_batch(values, None, None)
        .unwrap();
    col.close().unwrap();
    row_group.close().unwrap();
    writer.close().unwrap();
}

fn run_in(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_data-dict"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run data-dict")
}

// --- draft -----------------------------------------------------------------

/// `draft` writes ./data-dict.yaml by default, validates cleanly, appends new
/// tables on a second run, and reports when there is nothing to add.
#[test]
fn draft_writes_appends_and_reports_nothing_to_add() {
    let dir = temp_dir("draft-cycle");
    write_parquet(&dir.join("otters.parquet"), "otter_id", &[1, 1, 2, 2]);
    write_parquet(&dir.join("pups.parquet"), "pup_count", &[0, 1, 1]);

    let output = run_in(&dir, &["draft", "otters.parquet"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("added 1 table (otters)"), "{stdout}");
    let first = std::fs::read_to_string(dir.join("data-dict.yaml")).unwrap();
    assert!(first.contains("- name: otters"));

    let output = run_in(&dir, &["validate-spec", "data-dict.yaml"]);
    assert!(output.status.success(), "a fresh draft should validate");

    let output = run_in(&dir, &["draft", "otters.parquet", "pups.parquet"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stdout.contains("added 1 table (pups)"), "{stdout}");
    assert!(stderr.contains("skipped `otters`"), "{stderr}");
    let second = std::fs::read_to_string(dir.join("data-dict.yaml")).unwrap();
    assert!(
        second.starts_with(&first),
        "appending must preserve the existing text"
    );

    let output = run_in(&dir, &["draft", "otters.parquet"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("nothing to add"), "{stdout}");
    assert_eq!(
        std::fs::read_to_string(dir.join("data-dict.yaml")).unwrap(),
        second,
        "a no-op run must not rewrite the file"
    );
}

/// `-o -` prints the draft to stdout and writes no file.
#[test]
fn draft_to_stdout_writes_no_file() {
    let dir = temp_dir("draft-stdout");
    write_parquet(&dir.join("otters.parquet"), "otter_id", &[1, 1, 2, 2]);
    let output = run_in(&dir, &["draft", "otters.parquet", "-o", "-"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("$version:"), "{stdout}");
    assert!(!dir.join("data-dict.yaml").exists());
}

/// Two inputs with the same stem fail before anything is written.
#[test]
fn draft_duplicate_stems_fail() {
    let dir = temp_dir("draft-dup");
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();
    let output = run_in(&dir, &["draft", "a/otters.parquet", "b/otters.parquet"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("otters"), "{stderr}");
    assert!(!dir.join("data-dict.yaml").exists());
}

// --- describe ----------------------------------------------------------

/// `describe` prints the header and a per-column summary; `--json` carries
/// the same data structured.
#[test]
fn describe_text_and_json() {
    let dir = temp_dir("describe");
    write_parquet(&dir.join("pups.parquet"), "pup_count", &[0, 1, 1, 2]);

    let output = run_in(&dir, &["describe", "pups.parquet"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("pups.parquet: 4 rows \u{d7} 1 column"),
        "{stdout}"
    );
    assert!(stdout.contains("pup_count \u{2014} number"), "{stdout}");

    let output = run_in(&dir, &["describe", "pups.parquet", "--json"]);
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is valid JSON");
    assert_eq!(json["rows"], 4);
    assert_eq!(json["columns"][0]["name"], "pup_count");
    assert_eq!(json["columns"][0]["distinct"]["exact"], 3);
}

/// An unknown column errors and lists what is available.
#[test]
fn describe_unknown_column_fails_listing_columns() {
    let dir = temp_dir("describe-col");
    write_parquet(&dir.join("pups.parquet"), "pup_count", &[0, 1]);
    let output = run_in(&dir, &["describe", "pups.parquet", "wings"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("`wings` not found"), "{stderr}");
    assert!(stderr.contains("pup_count"), "{stderr}");
}

/// Only `.parquet` files can be described, and the error says so.
#[test]
fn describe_rejects_other_file_types() {
    let dir = temp_dir("describe-ext");
    std::fs::write(dir.join("pups.csv"), "pup_count\n1\n").unwrap();
    let output = run_in(&dir, &["describe", "pups.csv"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("only .parquet"), "{stderr}");
}

// --- export ------------------------------------------------------------

/// `export-spec` prints the resolved dictionary as one compact JSON line on
/// stdout (so it composes with tools like `jq`); `--pretty` pretty-prints it.
/// `export-data` profiles the source data into the same document.
#[test]
fn export_compact_and_pretty() {
    let dir = temp_dir("export");
    write_parquet(&dir.join("pups.parquet"), "pup_count", &[0, 1, 1, 2]);
    std::fs::write(
        dir.join("data-dict.yaml"),
        indoc::indoc! {"
            $version: \"0.1.0\"
            $learn_more: http://data-dict.tidyverse.org/
            tables:
              - name: pups
                source:
                  parquet: pups.parquet
                columns:
                  - name: pup_count
                    type: number
                    examples: [1]
        "},
    )
    .unwrap();

    let output = run_in(&dir, &["export-spec"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "compact output is one line");
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["tables"][0]["columns"][0]["name"], "pup_count");
    assert!(json["tables"][0]["columns"][0]["profile"].is_null());

    let output = run_in(&dir, &["export-spec", "--pretty"]);
    assert!(output.status.success());
    let pretty = String::from_utf8(output.stdout).unwrap();
    assert!(pretty.lines().count() > 1, "pretty output is multi-line");

    let output = run_in(&dir, &["export-data"]);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let profile = &json["tables"][0]["columns"][0]["profile"];
    assert_eq!(profile["distinct"]["count"], 3);
    assert_eq!(profile["missing"], 0);
}

/// An invalid dictionary exports nothing: diagnostics on stderr, nothing on
/// stdout, and a failing exit code.
#[test]
fn export_spec_fails_on_invalid_dictionary() {
    let fixture = multi_error_fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_data-dict"))
        .args(["export-spec"])
        .arg(&fixture)
        .output()
        .expect("failed to run data-dict");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "no document on stdout");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("S07"), "{stderr}");
}

// --- render ------------------------------------------------------------

/// A minimal dictionary whose one table declares `pups.parquet` as its source.
fn write_render_dict(dir: &std::path::Path, description: &str) {
    std::fs::write(
        dir.join("data-dict.yaml"),
        format!(
            indoc::indoc! {"
                $version: \"0.1.0\"
                $learn_more: http://data-dict.tidyverse.org/
                description: \"{}\"
                tables:
                  - name: pups
                    source:
                      parquet: pups.parquet
                    columns:
                      - name: pup_count
                        type: number
                        examples: [1]
            "},
            description
        ),
    )
    .unwrap();
}

/// With the source data present, `render` writes the page next to the
/// dictionary with the profiles embedded in its `#dict` document.
#[test]
fn render_profiles_data_into_the_page() {
    let dir = temp_dir("render-data");
    write_render_dict(&dir, "One row per pup.");
    write_parquet(&dir.join("pups.parquet"), "pup_count", &[0, 1, 1, 2]);

    let output = run_in(&dir, &["render"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("data-dict.html"), "{stdout}");

    let html = std::fs::read_to_string(dir.join("data-dict.html")).unwrap();
    assert!(html.contains(r#"<script type="application/json" id="dict">"#));
    assert!(html.contains(r#""distinct""#), "profiles are embedded");
}

/// With no source file present, `render` still writes the page — resolved
/// dictionary only, no profiles — and raises no missing-source warnings.
#[test]
fn render_without_data_is_quiet_and_unprofiled() {
    let dir = temp_dir("render-spec");
    write_render_dict(&dir, "One row per pup.");

    let output = run_in(&dir, &["render"]);
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "no warnings when no source is present: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let html = std::fs::read_to_string(dir.join("data-dict.html")).unwrap();
    assert!(html.contains(r#"<script type="application/json" id="dict">"#));
    assert!(
        !html.contains(r#""profile""#),
        "no profiles on the spec path"
    );
}

/// Unresolved `todo`s ride into the page so the flag beside a name has
/// something to show, and are still reported as warnings on the way.
#[test]
fn render_carries_todos_into_the_page() {
    let dir = temp_dir("render-todo");
    std::fs::write(
        dir.join("data-dict.yaml"),
        indoc::indoc! {"
            $version: \"0.1.0\"
            $learn_more: http://data-dict.tidyverse.org/
            description: Pups.
            todo: Confirm the years covered.
            tables:
              - name: pups
                todo: Check the grain.
                columns:
                  - name: pup_count
                    type: number
                    examples: [1]
                    todo: Add a description.
        "},
    )
    .unwrap();

    let output = run_in(&dir, &["render"]);
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("S31"), "{stderr}");

    // rendered to HTML, then embedded with every `<` as `\u003c`
    let html = std::fs::read_to_string(dir.join("data-dict.html")).unwrap();
    for todo in [
        r"\u003cp>Confirm the years covered.\u003c/p>",
        r"\u003cp>Check the grain.\u003c/p>",
        r"\u003cp>Add a description.\u003c/p>",
    ] {
        assert!(html.contains(todo), "missing {todo}");
    }
}

/// `-o` writes the page somewhere else.
#[test]
fn render_output_flag_overrides_the_path() {
    let dir = temp_dir("render-output");
    write_render_dict(&dir, "One row per pup.");

    let output = run_in(&dir, &["render", ".", "-o", "elsewhere.html"]);
    assert!(output.status.success());
    assert!(dir.join("elsewhere.html").is_file());
    assert!(!dir.join("data-dict.html").exists());
}

/// Dictionary text can't break out of the page's `#dict` script block: prose
/// has its raw HTML escaped when the Markdown is rendered, and every `<` that
/// remains in the JSON (a scalar, say) is embedded as `\u003c`.
#[test]
fn render_escapes_script_breaking_text() {
    let dir = temp_dir("render-escape");
    let payload = "</script><script>alert(1)</script>";
    std::fs::write(
        dir.join("data-dict.yaml"),
        format!(
            indoc::indoc! {"
                $version: \"0.1.0\"
                $learn_more: http://data-dict.tidyverse.org/
                description: \"{payload} pups\"
                tables:
                  - name: pups
                    columns:
                      - name: pup_name
                        type: string
                        examples: [\"{payload}\"]
            "},
            payload = payload
        ),
    )
    .unwrap();

    let output = run_in(&dir, &["render"]);
    assert!(output.status.success());
    let html = std::fs::read_to_string(dir.join("data-dict.html")).unwrap();
    assert!(
        !html.contains("</script><script>alert(1)"),
        "the payload must never appear raw"
    );
    // the description, through the Markdown renderer's escaping
    assert!(html.contains(r"&lt;/script&gt;&lt;script&gt;alert(1)"));
    // the example scalar, through the JSON `<` escape
    assert!(html.contains(r"\u003c/script>\u003cscript>alert(1)"));
}

/// An invalid dictionary renders nothing: diagnostics on stderr and no file
/// written, exactly as `export-spec` fails.
#[test]
fn render_fails_on_invalid_dictionary() {
    let dir = temp_dir("render-invalid");
    std::fs::write(
        dir.join("data-dict.yaml"),
        indoc::indoc! {"
            $version: \"0.1.0\"
            $learn_more: http://data-dict.tidyverse.org/
            tables:
              - name: pups
                columns:
                  - name: pup_count
                    type: number
        "},
    )
    .unwrap();

    let output = run_in(&dir, &["render"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("S07"), "{stderr}");
    assert!(!dir.join("data-dict.html").exists(), "nothing is written");
}

// --- validation report page --------------------------------------------

/// Write a one-column UTF-8 string parquet file at `path`.
fn write_string_parquet(path: &std::path::Path, column: &str, values: &[&str]) {
    use parquet::data_type::{ByteArray, ByteArrayType};
    use parquet::file::properties::WriterProperties;
    use parquet::file::writer::SerializedFileWriter;
    use parquet::schema::parser::parse_message_type;
    use std::sync::Arc;

    let message = format!("message schema {{ REQUIRED BYTE_ARRAY {column} (UTF8); }}");
    let schema = Arc::new(parse_message_type(&message).unwrap());
    let file = std::fs::File::create(path).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::new())).unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    let mut col = row_group.next_column().unwrap().unwrap();
    let values: Vec<ByteArray> = values.iter().map(|v| ByteArray::from(*v)).collect();
    col.typed::<ByteArrayType>()
        .write_batch(&values, None, None)
        .unwrap();
    col.close().unwrap();
    row_group.close().unwrap();
    writer.close().unwrap();
}

/// The two JSON payloads the page carries, unescaped and parsed.
fn page_payloads(html: &str) -> (serde_json::Value, String) {
    let payload = |id: &str| {
        let open = format!("<script type=\"application/json\" id=\"{id}\">");
        let start = html.find(&open).expect("the payload is in the page") + open.len();
        let end = start + html[start..].find("</script>").expect("the payload ends");
        html[start..end].replace("\\u003c", "<")
    };
    let report = serde_json::from_str(&payload("report")).expect("the report parses");
    let source = serde_json::from_str(&payload("source")).expect("the source parses");
    (report, source)
}

/// The page's report is the document `--json` prints, byte for byte: one run,
/// one serialization, two sinks.
#[test]
fn the_page_embeds_the_same_report_as_json() {
    let dir = temp_dir("html-same");
    write_render_dict(&dir, "One row per pup.");
    write_parquet(&dir.join("pups.parquet"), "pup_count", &[0, 1, 1, 2]);

    let page = dir.join("report.html");
    let output = run_in(
        &dir,
        &["validate-data", "--json", "--html", page.to_str().unwrap()],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    // `--json` owns stdout, so the note naming the page went to stderr.
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is one JSON");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("wrote "), "{stderr}");

    let html = std::fs::read_to_string(&page).unwrap();
    let (embedded, source) = page_payloads(&html);
    assert_eq!(embedded, report);
    assert_eq!(
        source,
        std::fs::read_to_string(dir.join("data-dict.yaml")).unwrap()
    );
}

/// A clean run is worth a page too: it says what was checked, not just what
/// went wrong.
#[test]
fn a_clean_run_still_writes_a_page() {
    let dir = temp_dir("html-clean");
    write_render_dict(&dir, "One row per pup.");
    write_parquet(&dir.join("pups.parquet"), "pup_count", &[0, 1, 1, 2]);

    let page = dir.join("report.html");
    let output = run_in(&dir, &["validate-data", "--html", page.to_str().unwrap()]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("wrote "), "{stdout}");

    let (report, _) = page_payloads(&std::fs::read_to_string(&page).unwrap());
    assert_eq!(report["status"], "ok");
    assert_eq!(report["problems"], serde_json::json!([]));
    let steps = report["steps"].as_array().expect("steps");
    assert!(steps.iter().all(|s| s["outcome"] == "pass"), "{steps:?}");
}

/// A spec-level run has no steps to list, and still writes a page and fails.
#[test]
fn a_failing_spec_run_writes_a_page() {
    let dir = temp_dir("html-spec");
    let page = dir.join("report.html");
    let fixture = multi_error_fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_data-dict"))
        .args(["validate-spec"])
        .arg(&fixture)
        .args(["--html", page.to_str().unwrap()])
        .output()
        .expect("failed to run data-dict");
    assert!(!output.status.success());

    let html = std::fs::read_to_string(&page).unwrap();
    let (report, source) = page_payloads(&html);
    assert_eq!(report["steps"], serde_json::json!([]));
    assert_eq!(report["run"]["level"], "spec");
    assert!(
        report["problems"]
            .as_array()
            .is_some_and(|p| p.iter().any(|problem| problem["code"] == "S07"))
    );
    assert_eq!(source, std::fs::read_to_string(&fixture).unwrap());
}

/// A run that never started has no report, so it has no page either.
#[test]
fn a_preflight_failure_writes_no_page() {
    let dir = temp_dir("html-preflight");
    let page = dir.join("report.html");
    let output = Command::new(env!("CARGO_BIN_EXE_data-dict"))
        .args(["validate-data"])
        .arg(dir.join("absent.yaml"))
        .args(["--html", page.to_str().unwrap()])
        .output()
        .expect("failed to run data-dict");
    assert!(!output.status.success());
    assert!(!page.exists(), "nothing is written");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no report written"), "{stderr}");
}

/// Nothing in the dictionary can close the page's `<script>` block, and a
/// marker spelled in the dictionary is embedded as written rather than expanded.
#[test]
fn the_page_escapes_script_breaking_yaml() {
    let dir = temp_dir("html-escape");
    let payload = "</script><script>alert(1)</script>";
    std::fs::write(
        dir.join("data-dict.yaml"),
        format!(
            indoc::indoc! {"
                $version: \"0.1.0\"
                $learn_more: http://data-dict.tidyverse.org/
                description: \"{payload} {{{{REPORT_JSON}}}} pups\"
                tables:
                  - name: pups
                    columns:
                      - name: pup_name
                        type: string
                        examples: [\"{payload}\"]
            "},
            payload = payload
        ),
    )
    .unwrap();

    let page = dir.join("report.html");
    let output = run_in(&dir, &["validate-spec", "--html", page.to_str().unwrap()]);
    assert!(output.status.success());
    let html = std::fs::read_to_string(&page).unwrap();
    assert!(
        !html.contains("</script><script>alert(1)"),
        "the payload must never appear raw"
    );
    // The dictionary's text rides along through the JSON `<` escape.
    assert!(html.contains(r"\u003c/script>\u003cscript>alert(1)"));
    // The source spells a document marker, which is embedded, never expanded.
    assert!(html.contains("{{REPORT_JSON}}"), "the marker was expanded");
    let (_, source) = page_payloads(&html);
    assert!(source.contains("{{REPORT_JSON}}"));
}

/// A restricted column's values are withheld from the report, so they never
/// reach the page either — the page renders what it is given and adds nothing.
#[test]
fn restricted_values_never_reach_the_page() {
    let dir = temp_dir("html-restricted");
    std::fs::write(
        dir.join("data-dict.yaml"),
        indoc::indoc! {"
            $version: \"0.1.0\"
            $learn_more: http://data-dict.tidyverse.org/
            description: One row per pup.
            tables:
              - name: pups
                source:
                  parquet: pups.parquet
                columns:
                  - name: carer
                    type: enum
                    display: restricted
                    values: [ada, grace]
        "},
    )
    .unwrap();
    write_string_parquet(&dir.join("pups.parquet"), "carer", &["ada", "hopper"]);

    let page = dir.join("report.html");
    let output = run_in(&dir, &["validate-data", "--html", page.to_str().unwrap()]);
    assert!(!output.status.success());
    let html = std::fs::read_to_string(&page).unwrap();
    assert!(
        !html.contains("hopper"),
        "a restricted column's offending value must not reach the page"
    );
    let (report, _) = page_payloads(&html);
    let problem = &report["problems"][0];
    assert_eq!(problem["code"], "D04");
    assert_eq!(problem["redacted"], true);
    // The rows stay, so the records are still findable by anyone entitled to them.
    assert_eq!(problem["rows"], serde_json::json!([2]));
}

/// A page that can't be written is a failure of the tool, not a verdict on the
/// dictionary, so a clean run still exits non-zero and says which path failed.
#[test]
fn a_page_that_cannot_be_written_fails_the_run() {
    let dir = temp_dir("html-unwritable");
    write_render_dict(&dir, "One row per pup.");
    let page = dir.join("absent-dir").join("report.html");

    let output = run_in(&dir, &["validate-spec", "--html", page.to_str().unwrap()]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(&page.display().to_string()), "{stderr}");
}
