//! Shared fixtures for the metadata and data comparison integration tests.
//!
//! Each helper writes a small parquet file and/or dictionary YAML to a unique
//! temp dir. Both `validate_meta` and `validate_data` test binaries include this
//! module, so not every helper is used by each — hence the blanket allow.
#![allow(dead_code)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use data_dict::RenderStyle;
use indoc::indoc;
use parquet::data_type::{ByteArray, ByteArrayType, DoubleType};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A unique temp directory for one test's fixtures.
pub fn temp_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "data-dict-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a parquet file with a string `name` column and a double `weight`
/// column.
pub fn write_parquet(path: &Path) {
    let message = indoc! {"
        message schema {
            REQUIRED BYTE_ARRAY name (UTF8);
            REQUIRED DOUBLE weight;
        }
    "};
    let schema = Arc::new(parse_message_type(message).unwrap());
    let props = Arc::new(WriterProperties::builder().build());
    let file = File::create(path).unwrap();
    let mut writer = SerializedFileWriter::new(file, schema, props).unwrap();
    let mut rg = writer.next_row_group().unwrap();

    let mut col = rg.next_column().unwrap().unwrap();
    col.typed::<ByteArrayType>()
        .write_batch(
            &[ByteArray::from("otter"), ByteArray::from("seal")],
            None,
            None,
        )
        .unwrap();
    col.close().unwrap();

    let mut col = rg.next_column().unwrap().unwrap();
    col.typed::<DoubleType>()
        .write_batch(&[1.0_f64, 2.0], None, None)
        .unwrap();
    col.close().unwrap();

    rg.close().unwrap();
    writer.close().unwrap();
}

/// Write a parquet file with nested columns, for the struct/list checks:
///
/// | row | `name` (string) | `addr` (struct{zip, country})   | `tags` (list of string) |
/// |-----|-----------------|---------------------------------|-------------------------|
/// | 1   | otter           | {zip: "97201", country: "US"}   | [a, b]                  |
/// | 2   | seal            | {zip: "78701", country: "XX"}   | [a, zz]                 |
/// | 3   | mink            | null                            | null                    |
pub fn write_nested_parquet(path: &Path) {
    use arrow_array::builder::{ListBuilder, StringBuilder};
    use arrow_array::{ArrayRef, RecordBatch, StringArray, StructArray};
    use arrow_buffer::NullBuffer;
    use arrow_schema::{DataType, Field, Fields};
    use parquet::arrow::ArrowWriter;

    let name = StringArray::from(vec!["otter", "seal", "mink"]);

    let zip = StringArray::from(vec![Some("97201"), Some("78701"), None]);
    let country = StringArray::from(vec![Some("US"), Some("XX"), None]);
    let addr_fields: Fields = vec![
        Arc::new(Field::new("zip", DataType::Utf8, true)),
        Arc::new(Field::new("country", DataType::Utf8, true)),
    ]
    .into();
    let addr = StructArray::new(
        addr_fields,
        vec![Arc::new(zip) as ArrayRef, Arc::new(country) as ArrayRef],
        Some(NullBuffer::from(vec![true, true, false])),
    );

    let mut tags = ListBuilder::new(StringBuilder::new());
    tags.values().append_value("a");
    tags.values().append_value("b");
    tags.append(true);
    tags.values().append_value("a");
    tags.values().append_value("zz");
    tags.append(true);
    tags.append(false);
    let tags = tags.finish();

    let batch = RecordBatch::try_from_iter(vec![
        ("name", Arc::new(name) as ArrayRef),
        ("addr", Arc::new(addr) as ArrayRef),
        ("tags", Arc::new(tags) as ArrayRef),
    ])
    .unwrap();

    let file = File::create(path).unwrap();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}

/// Write a parquet file with one `grid` column of type list-of-list-of-string:
///
/// | row | `grid`             |
/// |-----|--------------------|
/// | 1   | [[a, b]]           |
/// | 2   | [[a], [zz]]        |
/// | 3   | null               |
pub fn write_matrix_parquet(path: &Path) {
    use arrow_array::builder::{ListBuilder, StringBuilder};
    use arrow_array::{ArrayRef, RecordBatch};
    use parquet::arrow::ArrowWriter;

    let mut grid = ListBuilder::new(ListBuilder::new(StringBuilder::new()));
    grid.values().values().append_value("a");
    grid.values().values().append_value("b");
    grid.values().append(true);
    grid.append(true);
    grid.values().values().append_value("a");
    grid.values().append(true);
    grid.values().values().append_value("zz");
    grid.values().append(true);
    grid.append(true);
    grid.append(false);
    let grid = grid.finish();

    let batch = RecordBatch::try_from_iter(vec![("grid", Arc::new(grid) as ArrayRef)]).unwrap();
    let file = File::create(path).unwrap();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}

/// Write `yaml` to `<dir>/dict.yaml` and return the path.
pub fn write_yaml(dir: &Path, yaml: &str) -> PathBuf {
    let path = dir.join("dict.yaml");
    std::fs::write(&path, yaml).unwrap();
    path
}

/// The boilerplate top-level header — the required `$version` and recommended
/// `$learn_more` — that nearly every dictionary needs. Exactly two lines, so a
/// `body` written beneath it starts at line 3.
pub const HEADER: &str = "$version: \"0.1.0\"\n$learn_more: http://data-dict.tidyverse.org/\n";

/// Write `body` to `<dir>/dict.yaml` with [`HEADER`] prepended, returning the
/// path, so a test shows only the part under test. Use [`write_yaml`] for the
/// few cases that must omit or alter the header keys themselves.
pub fn write_dict(dir: &Path, body: &str) -> PathBuf {
    write_yaml(dir, &format!("{HEADER}{body}"))
}

/// Render diagnostics for snapshots: no colour (so no escape codes to strip)
/// and anonymized line numbers (so an edit that shifts lines doesn't churn
/// unrelated snapshots).
pub const SNAPSHOT_STYLE: RenderStyle = RenderStyle {
    color: false,
    anonymized_line_numbers: true,
};

/// Make a source-highlighted diagnostic snapshottable: rewrite the absolute
/// `dir` prefix off its paths, which varies per run. `dir` is the temp dir for
/// inline documents (leaving the bare `dict.yaml`) or the fixtures root for
/// fixture documents (leaving e.g. `spec/s01-….yaml`). Backslashes are
/// normalized to `/` so Windows paths match the snapshots. Render with
/// [`SNAPSHOT_STYLE`] so there is no terminal styling to strip.
pub fn sanitize(rendered: &str, dir: &Path) -> String {
    let dir_prefix = format!("{}/", dir.display()).replace('\\', "/");
    rendered.replace('\\', "/").replace(&dir_prefix, "")
}

/// A validation outcome captured for snapshotting: the YAML `source` that was
/// validated and the `rendered` diagnostic it produced. Snapshot it with
/// [`assert_snapshot!`], which shows the source and the diagnostic together.
pub struct Diagnostic {
    pub source: String,
    pub rendered: String,
}

/// Separates the validated YAML source from the rendered diagnostic in a
/// combined snapshot body.
const RULE: &str = "────────────────────────────────────────";

impl Diagnostic {
    /// The snapshot body: the YAML source, a rule, then the rendered diagnostic.
    pub fn body(&self) -> String {
        format!("{}{RULE}\n{}", self.source, self.rendered)
    }

    /// Assert the rendered diagnostic contains every fragment in `expected`.
    /// Runs unconditionally, so it carries the cross-platform error coverage on
    /// the platforms where the (Unix-only) snapshot does not run.
    pub fn assert_contains(&self, expected: &[&str]) {
        for fragment in expected {
            assert!(
                self.rendered.contains(fragment),
                "expected {fragment:?} in diagnostic, got:\n{}",
                self.rendered,
            );
        }
    }
}

/// Capture a validation result for snapshotting: read the validated YAML from
/// `path` as the source and [`sanitize`] `rendered` against the file's own
/// directory (turning its absolute paths into the bare `dict.yaml`).
pub fn diagnostic(path: &Path, rendered: &str) -> Diagnostic {
    Diagnostic {
        source: std::fs::read_to_string(path).unwrap(),
        rendered: sanitize(rendered, path.parent().unwrap()),
    }
}

/// Snapshot a [`Diagnostic`] as its YAML source followed by the rendered
/// diagnostic (see [`Diagnostic::body`]), so the `.snap` shows input and output
/// together. The source lives in the snapshot body rather than metadata, so it
/// is readable and self-maintaining: editing the YAML shows up as an ordinary
/// snapshot diff. The redundant `expression:` header is omitted.
macro_rules! assert_snapshot {
    ($diagnostic:expr) => {{
        let body = $diagnostic.body();
        let mut settings = insta::Settings::clone_current();
        settings.set_omit_expression(true);
        let _guard = settings.bind_to_scope();
        insta::assert_snapshot!(body);
    }};
}
pub(crate) use assert_snapshot;
