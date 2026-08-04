//! Integration tests that run the `data-dict` binary end to end.

use std::path::PathBuf;
use std::process::Command;

/// Running `data-dict` with no arguments lists every subcommand, including
/// nested ones like `skill read`.
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

// --- describe ----------------------------------------------------------

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
