//! `draft` generation: inference, emission, and append mode. Every generated
//! dictionary must validate with no errors — that is the command's core
//! promise. The only expected findings are the S31 warnings raised by the
//! draft's own `todo` notes.

mod common;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::temp_dir;
use data_dict::draft::{DraftError, draft};
use parquet::data_type::{BoolType, ByteArray, ByteArrayType, DoubleType, Int32Type, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;

/// One column's values, `None` being a null.
enum Col {
    I32(Vec<Option<i32>>),
    I64(Vec<Option<i64>>),
    F64(Vec<Option<f64>>),
    Bool(Vec<Option<bool>>),
    Str(Vec<Option<String>>),
}

fn required<T>(values: impl IntoIterator<Item = T>) -> Vec<Option<T>> {
    values.into_iter().map(Some).collect()
}

fn strings<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<Option<String>> {
    values.into_iter().map(|v| Some(v.to_string())).collect()
}

/// Write a parquet file at `path` whose schema is `fields` (Parquet schema
/// lines such as `"REQUIRED INT64 id"`), one row group.
fn write_parquet(path: &Path, fields: &[&str], columns: Vec<Col>) {
    let message = format!("message schema {{ {}; }}", fields.join("; "));
    let schema = Arc::new(parse_message_type(&message).unwrap());
    let optional: Vec<bool> = fields.iter().map(|f| f.contains("OPTIONAL")).collect();
    let file = File::create(path).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::new())).unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    for (values, optional) in columns.iter().zip(&optional) {
        let mut column = row_group.next_column().unwrap().unwrap();
        // Write the present values with definition levels marking the nulls.
        macro_rules! write {
            ($type:ty, $items:expr, $map:expr) => {{
                let levels: Vec<i16> = $items.iter().map(|v| v.is_some() as i16).collect();
                let present: Vec<_> = $items.iter().flatten().map($map).collect();
                let levels = optional.then_some(levels.as_slice());
                column
                    .typed::<$type>()
                    .write_batch(&present, levels, None)
                    .unwrap();
            }};
        }
        match values {
            Col::I32(items) => write!(Int32Type, items, |v| *v),
            Col::I64(items) => write!(Int64Type, items, |v| *v),
            Col::F64(items) => write!(DoubleType, items, |v| *v),
            Col::Bool(items) => write!(BoolType, items, |v| *v),
            Col::Str(items) => write!(ByteArrayType, items, |v| ByteArray::from(v.as_str())),
        }
        column.close().unwrap();
    }
    row_group.close().unwrap();
    writer.close().unwrap();
}

/// A file whose only column sits inside a group, which the profiler can't read.
fn write_nested(path: &Path) {
    let schema = Arc::new(
        parse_message_type("message schema { OPTIONAL group readings { REQUIRED INT64 v; } }")
            .unwrap(),
    );
    let file = File::create(path).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::new())).unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    let mut column = row_group.next_column().unwrap().unwrap();
    column
        .typed::<Int64Type>()
        .write_batch(&[1, 2, 3], Some(&[1, 1, 1]), None)
        .unwrap();
    column.close().unwrap();
    row_group.close().unwrap();
    writer.close().unwrap();
}

/// The command's core promise: every draft passes `validate-spec` with no
/// errors; the only findings are the S31 warnings its own `todo` notes raise.
fn assert_spec_clean(content: &str) {
    let problems = data_dict::validate_spec_str(content, "draft.yaml");
    let unexpected: Vec<String> = problems
        .items
        .iter()
        .filter(|p| p.code != Some("S31"))
        .map(|p| p.to_text(&problems.source, common::SNAPSHOT_STYLE))
        .collect();
    assert!(
        unexpected.is_empty(),
        "draft should validate with only S31 todo warnings, got:\n{}\n---\n{content}",
        unexpected.join("\n"),
    );
}

fn draft_new(dir: &Path, inputs: &[PathBuf]) -> String {
    let outcome = draft(inputs, dir, None).unwrap();
    assert_spec_clean(&outcome.content);
    outcome.content
}

#[test]
fn drafts_enum_required_and_example_todos() {
    let dir = temp_dir();
    let path = dir.join("penguins.parquet");
    write_parquet(
        &path,
        &[
            "REQUIRED BYTE_ARRAY species (UTF8)",
            "OPTIONAL DOUBLE bill_length_mm",
            "REQUIRED INT64 body_mass_g",
            "OPTIONAL BYTE_ARRAY sex (UTF8)",
        ],
        vec![
            Col::Str(strings([
                "Adelie",
                "Adelie",
                "Adelie",
                "Adelie",
                "Chinstrap",
                "Chinstrap",
                "Chinstrap",
                "Gentoo",
                "Gentoo",
            ])),
            Col::F64(vec![
                Some(39.1),
                Some(39.5),
                None,
                Some(36.7),
                Some(46.5),
                Some(50.0),
                Some(45.8),
                Some(48.7),
                Some(59.6),
            ]),
            Col::I64(required([
                3750, 3800, 3250, 3450, 3650, 3625, 4675, 4850, 6300,
            ])),
            Col::Str(vec![
                Some("male".to_string()),
                Some("female".to_string()),
                None,
                Some("female".to_string()),
                Some("male".to_string()),
                Some("male".to_string()),
                None,
                Some("female".to_string()),
                Some("male".to_string()),
            ]),
        ],
    );
    insta::assert_snapshot!(draft_new(&dir, &[path]));
}

#[test]
fn drafts_id_types_unique_todos_and_key_candidates() {
    let dir = temp_dir();
    let path = dir.join("bands.parquet");
    write_parquet(
        &path,
        &[
            "REQUIRED INT64 id",
            "REQUIRED INT32 site_id",
            "REQUIRED BYTE_ARRAY tag (UTF8)",
        ],
        vec![
            Col::I64(required([1, 2, 3, 4, 5, 6, 7, 8])),
            Col::I32(required([10, 10, 20, 20, 30, 30, 40, 40])),
            Col::Str(strings([
                "AA-01", "AA-02", "BB-01", "BB-02", "CC-01", "CC-02", "DD-01", "DD-02",
            ])),
        ],
    );
    insta::assert_snapshot!(draft_new(&dir, &[path]));
}

/// A column left as a bare `number` is drafted with both `range` and
/// `examples`, because the data can't say whether it holds ordinals or
/// quantities (which take a range) or bare numbers (which take examples); the
/// todo asks for the type and which key to keep. Nothing else takes the pair: a
/// `number(id)` has already settled its type, a `string` takes `examples`
/// alone, and a `boolean` neither.
#[test]
fn drafts_both_range_and_examples_for_bare_number_columns() {
    let dir = temp_dir();
    let path = dir.join("readings.parquet");
    write_parquet(
        &path,
        &[
            "REQUIRED DOUBLE temperature",
            "REQUIRED INT64 station_id",
            "REQUIRED BYTE_ARRAY station (UTF8)",
            "REQUIRED BOOLEAN checked",
        ],
        vec![
            Col::F64(required([1.5, 3.0, 7.25, 9.5])),
            Col::I64(required([1, 2, 3, 4])),
            Col::Str(strings(["kew", "leuchars", "eskdalemuir", "lerwick"])),
            Col::Bool(required([true, false, true, false])),
        ],
    );
    let content = draft_new(&dir, &[path]);

    assert!(content.contains("range: [1.5, 9.5]"), "{content}");
    assert!(
        content.contains("examples: [1.5, 3, 7.25, 9.5]"),
        "{content}"
    );
    assert!(
        content.contains("Pick a more specific numeric type"),
        "the pair needs a todo asking which to keep:\n{content}"
    );

    // Only the bare `number` gets a range: `station_id` drafts as
    // `number(id)`, `station` is a string, and `checked` a boolean.
    assert_eq!(content.matches("range:").count(), 1, "{content}");

    insta::assert_snapshot!(content);
}

#[test]
fn drafts_temporal_ranges_and_time_zones() {
    let dir = temp_dir();
    let path = dir.join("sightings.parquet");
    write_parquet(
        &path,
        &[
            "REQUIRED INT32 event_date (DATE)",
            "REQUIRED INT64 seen_utc (TIMESTAMP(MICROS,true))",
            "REQUIRED INT64 seen_local (TIMESTAMP(MILLIS,false))",
        ],
        vec![
            // 2020-01-01 .. 2024-01-01, with repeats so nothing reads unique.
            Col::I32(required([18262, 18262, 18628, 19000, 19723, 19723])),
            Col::I64(required([
                1_577_836_800_000_000, // 2020-01-01T00:00:00
                1_577_836_800_000_000,
                1_600_000_000_000_000,
                1_600_000_000_000_000,
                1_704_067_199_000_000, // 2023-12-31T23:59:59
                1_704_067_199_000_000,
            ])),
            Col::I64(required([
                1_577_865_600_123, // 2020-01-01T08:00:00.123
                1_577_865_600_123,
                1_600_000_000_000,
                1_600_000_000_000,
                1_704_096_000_000, // 2024-01-01T08:00:00
                1_704_096_000_000,
            ])),
        ],
    );
    insta::assert_snapshot!(draft_new(&dir, &[path]));
}

#[test]
fn drafts_boolean_bare_and_unprofilable_name_only() {
    let dir = temp_dir();
    let bools = dir.join("flags.parquet");
    write_parquet(
        &bools,
        &["REQUIRED BOOLEAN alive"],
        vec![Col::Bool(required([true, false, true, true]))],
    );
    let nested = dir.join("telemetry.parquet");
    write_nested(&nested);
    insta::assert_snapshot!(draft_new(&dir, &[bools, nested]));
}

#[test]
fn long_values_fall_back_to_block_lists_and_quoting() {
    let dir = temp_dir();
    let path = dir.join("habitats.parquet");
    let descriptions = [
        "rocky intertidal zone with extensive kelp forest canopy",
        "sheltered estuarine mudflats fringed by eelgrass meadows",
        "exposed offshore reef with strong tidal currents year-round",
        "protected harbor with artificial breakwaters and pilings",
    ];
    let habitat: Vec<&str> = descriptions.iter().cycle().take(12).copied().collect();
    write_parquet(
        &path,
        &[
            "REQUIRED BYTE_ARRAY habitat (UTF8)",
            "REQUIRED BYTE_ARRAY zip (UTF8)",
        ],
        vec![
            Col::Str(strings(habitat)),
            // Read as numbers unless the draft quotes them (spec rule S12).
            Col::Str(strings([
                "02134", "02134", "02134", "94110", "94110", "94110", "60614", "60614", "60614",
                "73301", "73301", "73301",
            ])),
        ],
    );
    let content = draft_new(&dir, &[path]);
    assert!(
        content.lines().all(|l| l.len() <= 80),
        "no generated line should overrun 80 columns:\n{content}"
    );
    insta::assert_snapshot!(content);
}

#[test]
fn approximate_distinct_counts_soften_the_wording() {
    let dir = temp_dir();
    let path = dir.join("visits.parquet");
    // Enough distinct values to overflow the exact tracker, so the distinct
    // count is a sketch estimate rather than a proven fact.
    let codes: Vec<String> = (0..1200).map(|i| format!("visit-{i:04}")).collect();
    write_parquet(
        &path,
        &["REQUIRED BYTE_ARRAY visit_code (UTF8)"],
        vec![Col::Str(codes.iter().map(|c| Some(c.clone())).collect())],
    );
    let content = draft_new(&dir, &[path]);
    assert!(
        content.contains("approximate"),
        "an approximate distinct == rows should be hedged:\n{content}"
    );
    insta::assert_snapshot!(content);
}

#[test]
fn drafts_multiple_tables_in_argument_order() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    let first = dir.join("data/otters.parquet");
    let second = dir.join("data/pups.parquet");
    write_parquet(
        &first,
        &["REQUIRED INT64 otter_id"],
        vec![Col::I64(required([1, 1, 2, 2]))],
    );
    write_parquet(
        &second,
        &["REQUIRED INT64 pup_count"],
        vec![Col::I64(required([0, 1, 1, 2]))],
    );
    insta::assert_snapshot!(draft_new(&dir, &[first, second]));
}

#[test]
fn duplicate_stems_error_before_writing() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();
    let first = dir.join("a/otters.parquet");
    let second = dir.join("b/otters.parquet");
    // Never even created: stems collide before any file is opened.
    let err = draft(&[first, second], &dir, None).unwrap_err();
    assert!(matches!(err, DraftError::DuplicateStem { .. }));
    assert!(err.to_string().contains("otters"), "{err}");
}

#[test]
fn missing_input_reports_its_path() {
    let dir = temp_dir();
    let path = dir.join("absent.parquet");
    let err = draft(std::slice::from_ref(&path), &dir, None).unwrap_err();
    assert!(matches!(err, DraftError::Parquet { .. }));
    assert!(err.to_string().contains("absent.parquet"), "{err}");
}

// --- append mode ---------------------------------------------------------

/// An existing dictionary with hand-written quirks — comments, a trailing
/// glossary — that appending must preserve byte for byte.
const EXISTING: &str = "\
# My hand-written dictionary.
$version: 0.1.0
$learn_more: http://data-dict.tidyverse.org/

tables:
  - name: otters   # the flagship table
    source:
      parquet: data/otters.parquet
    columns:
      - name: otter_id
        type: number(id)
        examples: [1, 2, 3]

glossary:
  pup: An otter younger than one year.
";

fn pups_parquet(dir: &Path) -> PathBuf {
    let path = dir.join("pups.parquet");
    write_parquet(
        &path,
        &["REQUIRED INT64 pup_count"],
        vec![Col::I64(required([0, 1, 1, 2]))],
    );
    path
}

#[test]
fn append_inserts_after_last_table_and_preserves_bytes() {
    let dir = temp_dir();
    let path = pups_parquet(&dir);
    let outcome = draft(&[path], &dir, Some(EXISTING)).unwrap();
    assert_eq!(outcome.added, ["pups"]);
    assert!(outcome.skipped.is_empty());

    // Every original byte survives: the content is the original with one
    // contiguous block inserted.
    let split = outcome
        .content
        .find("\n  - name: pups")
        .expect("the new table should be present");
    let head = &outcome.content[..split];
    let inserted_end = split + outcome.content.len() - EXISTING.len();
    let tail = &outcome.content[inserted_end..];
    assert_eq!(format!("{head}{tail}"), EXISTING);
    assert_spec_clean(&outcome.content);
    insta::assert_snapshot!(outcome.content);
}

#[test]
fn append_skips_tables_already_present() {
    let dir = temp_dir();
    let otters = dir.join("otters.parquet");
    write_parquet(
        &otters,
        &["REQUIRED INT64 otter_id"],
        vec![Col::I64(required([1, 2]))],
    );
    let pups = pups_parquet(&dir);
    let outcome = draft(&[otters, pups], &dir, Some(EXISTING)).unwrap();
    assert_eq!(outcome.added, ["pups"]);
    assert_eq!(outcome.skipped, ["otters"]);

    let outcome = draft(&[pups_parquet(&dir)], &dir, Some(&outcome.content)).unwrap();
    assert!(outcome.added.is_empty());
    assert_eq!(outcome.skipped, ["pups"]);
}

#[test]
fn append_with_nothing_new_leaves_content_untouched() {
    let dir = temp_dir();
    let otters = dir.join("otters.parquet");
    write_parquet(
        &otters,
        &["REQUIRED INT64 otter_id"],
        vec![Col::I64(required([1, 2]))],
    );
    let outcome = draft(&[otters], &dir, Some(EXISTING)).unwrap();
    assert!(outcome.added.is_empty());
    assert_eq!(outcome.content, EXISTING);
}

#[test]
fn append_matches_the_existing_indent() {
    let dir = temp_dir();
    let existing = "\
$version: 0.1.0
tables:
    - name: otters
      source:
        parquet: data/otters.parquet
      columns:
        - name: otter_id
          type: number(id)
          examples: [1, 2, 3]
";
    let path = pups_parquet(&dir);
    let outcome = draft(&[path], &dir, Some(existing)).unwrap();
    assert!(
        outcome
            .content
            .contains("\n    - name: pups\n      source:"),
        "the appended entry should use the file's 4-space item indent:\n{}",
        outcome.content
    );
}

#[test]
fn append_without_tables_key_starts_the_section() {
    let dir = temp_dir();
    let existing = "$version: 0.1.0\n$learn_more: http://data-dict.tidyverse.org/\n";
    let path = pups_parquet(&dir);
    let outcome = draft(&[path], &dir, Some(existing)).unwrap();
    assert!(outcome.content.starts_with(existing));
    assert_spec_clean(&outcome.content);
    insta::assert_snapshot!(outcome.content);
}

#[test]
fn append_to_empty_tables_list_errors() {
    let dir = temp_dir();
    let path = pups_parquet(&dir);
    let err = draft(&[path], &dir, Some("$version: 0.1.0\ntables: []\n")).unwrap_err();
    assert!(matches!(err, DraftError::EmptyTables));
}

#[test]
fn append_to_invalid_dictionary_errors() {
    let dir = temp_dir();
    let path = pups_parquet(&dir);
    let err = draft(&[path], &dir, Some("tables: [:::\n")).unwrap_err();
    assert!(matches!(err, DraftError::ExistingInvalid { .. }));
    assert!(err.to_string().contains("validate-spec"), "{err}");
}
