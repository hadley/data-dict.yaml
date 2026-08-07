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
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    insta::assert_snapshot!(serde_json::to_string_pretty(&value).unwrap());
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
