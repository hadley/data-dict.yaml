//! Integration tests for the data level (`data_dict::validate_data`): the
//! data's *values* against the dictionary, which requires scanning the data.
//!
//! These tests focus on the value-level checks the data level adds on top of the
//! metadata checks (today, nulls in a required column), and confirm the metadata
//! and data levels are genuinely distinct.

mod common;
use common::{assert_snapshot, temp_dir, write_dict};

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use data_dict::{Problem, ProblemKind, ProblemSet, Status, validate_data, validate_meta};
use indoc::{formatdoc, indoc};
use parquet::data_type::{
    ByteArray, ByteArrayType, DoubleType, FixedLenByteArray, FixedLenByteArrayType, Int32Type,
    Int64Type,
};
use parquet::file::properties::{EnabledStatistics, WriterProperties};
use parquet::file::writer::{SerializedColumnWriter, SerializedFileWriter};
use parquet::schema::parser::parse_message_type;

/// Validate a single column's values in isolation, via [`build_column`].
fn check_column(
    schema_col: &str,
    write: impl FnOnce(&mut SerializedColumnWriter),
    column: &str,
) -> ProblemSet {
    let yaml = build_column(schema_col, write, column);
    validate_data(&yaml, None)
}

/// Write a one-column parquet file (`schema_col` is that column's line in a
/// parquet message-type schema, e.g. `OPTIONAL DOUBLE weight`; `write` fills in
/// its data) and wrap `column` — the YAML for one `columns:` entry — in an
/// otherwise-minimal one-table dictionary whose `source` points at that file.
/// Returns the dictionary path.
fn build_column(
    schema_col: &str,
    write: impl FnOnce(&mut SerializedColumnWriter),
    column: &str,
) -> PathBuf {
    build_column_with_properties(
        schema_col,
        write,
        column,
        WriterProperties::builder().build(),
    )
}

fn build_column_with_properties(
    schema_col: &str,
    write: impl FnOnce(&mut SerializedColumnWriter),
    column: &str,
    properties: WriterProperties,
) -> PathBuf {
    let dir = temp_dir();
    let parquet = dir.join("data.parquet");

    let message = format!("message schema {{ {schema_col}; }}");
    let schema = Arc::new(parse_message_type(&message).unwrap());
    let props = Arc::new(properties);
    let file = File::create(&parquet).unwrap();
    let mut writer = SerializedFileWriter::new(file, schema, props).unwrap();
    let mut rg = writer.next_row_group().unwrap();
    let mut col = rg.next_column().unwrap().unwrap();
    write(&mut col);
    col.close().unwrap();
    rg.close().unwrap();
    writer.close().unwrap();

    // Indent the caller's column entry to sit under `columns:`.
    let column = column
        .trim_end()
        .lines()
        .map(|line| format!("      {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    write_dict(
        &dir,
        &formatdoc! {"
            tables:
              - name: t
                source:
                  parquet: data.parquet
                columns:
            {column}
        "},
    )
}

/// Write an optional double column whose second row (1-based) is null.
fn write_double_with_null(col: &mut SerializedColumnWriter) {
    // Definition levels: 1 = present, 0 = null. Row 2 is null, so the values
    // slice holds only the two non-null doubles.
    col.typed::<DoubleType>()
        .write_batch(&[1.0_f64, 2.0], Some(&[1, 0, 1]), None)
        .unwrap();
}

fn build_composite_key(first: &[f64], second: &[f64]) -> PathBuf {
    let dir = temp_dir();
    let parquet = dir.join("data.parquet");
    let schema = Arc::new(
        parse_message_type("message schema { REQUIRED DOUBLE a; REQUIRED DOUBLE b; }").unwrap(),
    );
    let file = File::create(&parquet).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build()))
            .unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    let mut a = row_group.next_column().unwrap().unwrap();
    a.typed::<DoubleType>()
        .write_batch(first, None, None)
        .unwrap();
    a.close().unwrap();
    let mut b = row_group.next_column().unwrap().unwrap();
    b.typed::<DoubleType>()
        .write_batch(second, None, None)
        .unwrap();
    b.close().unwrap();
    row_group.close().unwrap();
    writer.close().unwrap();

    write_dict(
        &dir,
        indoc! {"
            tables:
              - name: t
                source:
                  parquet: data.parquet
                columns:
                  - name: a
                    type: number(id)
                    constraints: [primary_key]
                    examples: [1, 2]
                  - name: b
                    type: number(id)
                    constraints: [primary_key]
                    examples: [1, 2]
        "},
    )
}

/// The defining difference between the two levels: a `required` column with
/// nulls is a *value* problem, so it is invisible to `validate-meta` (which
/// reads only names and types) but caught by `validate-data` (which scans).
#[test]
fn meta_ignores_null_values_that_data_catches() {
    let yaml = build_column(
        "OPTIONAL DOUBLE weight",
        write_double_with_null,
        indoc! {"
            - name: weight
              type: number(quantity)
              constraints: [required]
              range: [0, 100]
        "},
    );

    // Metadata level: the column exists with a compatible type, so it's clean.
    let meta = validate_meta(&yaml, None);
    assert_eq!(meta.status(), Status::Ok, "meta got {:?}", meta.items);

    // Data level: the null in a required column is an error.
    let data = validate_data(&yaml, None);
    assert_eq!(data.status(), Status::Error);
    assert!(
        matches!(
            data.items.as_slice(),
            [Problem {
                code: Some(code),
                kind: ProblemKind::NullsInRequired { .. },
                ..
            }] if *code == "D01"
        ),
        "data got {:?}",
        data.items
    );
}

#[test]
fn nulls_in_required_column_reported() {
    let yaml = build_column(
        "OPTIONAL DOUBLE weight",
        write_double_with_null,
        indoc! {"
            - name: weight
              type: number(quantity)
              constraints: [required]
              range: [0, 100]
        "},
    );
    let result = validate_data(&yaml, None);

    assert_eq!(result.status(), Status::Error);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem { kind: ProblemKind::NullsInRequired { count, rows }, .. }]
                if *count == 1 && rows.is_empty()
        ),
        "got {:?}",
        result.items
    );
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(
        &yaml,
        &result.render(common::SNAPSHOT_STYLE).join("\n")
    ));
}

#[test]
fn missing_null_statistics_falls_back_to_data_scan() {
    let yaml = build_column_with_properties(
        "OPTIONAL DOUBLE weight",
        write_double_with_null,
        indoc! {"
            - name: weight
              type: number(quantity)
              constraints: [required]
              range: [0, 100]
        "},
        WriterProperties::builder()
            .set_statistics_enabled(EnabledStatistics::None)
            .build(),
    );

    let result = validate_data(&yaml, None);
    assert!(matches!(
        result.items.as_slice(),
        [Problem { kind: ProblemKind::NullsInRequired { count: 1, rows }, .. }]
            if rows == &[2]
    ));
}

#[test]
fn required_column_without_nulls_ok() {
    // No nulls present, so the statistics fast-path should resolve this without
    // scanning the data pages.
    let result = check_column(
        "REQUIRED DOUBLE weight",
        |col| {
            col.typed::<DoubleType>()
                .write_batch(&[1.0_f64, 2.0, 3.0], None, None)
                .unwrap();
        },
        indoc! {"
            - name: weight
              type: number(quantity)
              constraints: [required]
              range: [0, 100]
        "},
    );

    assert_eq!(result.status(), Status::Ok);
}

#[test]
fn nulls_in_optional_column_ok() {
    // `weight` has a null but is not declared required, so it's fine.
    let result = check_column(
        "OPTIONAL DOUBLE weight",
        write_double_with_null,
        indoc! {"
            - name: weight
              type: number(quantity)
              range: [0, 100]
        "},
    );

    assert_eq!(result.status(), Status::Ok);
}

/// Write the given strings as a required UTF-8 byte-array column.
fn write_strings<'a>(values: &'a [&'a str]) -> impl FnOnce(&mut SerializedColumnWriter) + 'a {
    move |col| {
        let bytes = values
            .iter()
            .map(|s| ByteArray::from(*s))
            .collect::<Vec<_>>();
        col.typed::<ByteArrayType>()
            .write_batch(&bytes, None, None)
            .unwrap();
    }
}

#[test]
fn values_outside_enum_reported() {
    let yaml = build_column(
        "REQUIRED BYTE_ARRAY status (UTF8)",
        write_strings(&["active", "banned", "active", "sleepy"]),
        indoc! {"
            - name: status
              type: enum
              values: [active, banned]
        "},
    );
    let result = validate_data(&yaml, None);

    assert_eq!(result.status(), Status::Error);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D04"),
                kind: ProblemKind::ValuesOutsideEnum { count: 1, rows, values },
                ..
            }] if rows == &[4] && values == &["sleepy"]
        ),
        "got {:?}",
        result.items
    );
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(
        &yaml,
        &result.render(common::SNAPSHOT_STYLE).join("\n")
    ));
}

/// A restricted column's offending values are withheld from both the message
/// and the structured kind.
#[test]
fn values_outside_enum_withheld_when_restricted() {
    let yaml = build_column(
        "REQUIRED BYTE_ARRAY status (UTF8)",
        write_strings(&["active", "banned", "active", "sleepy"]),
        indoc! {"
            - name: status
              type: enum
              values: [active, banned]
              display: restricted
        "},
    );
    let result = validate_data(&yaml, None);

    assert_eq!(result.status(), Status::Error);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D04"),
                kind: ProblemKind::ValuesOutsideEnum { count: 1, rows, values },
                ..
            }] if rows == &[4] && values.is_empty()
        ),
        "got {:?}",
        result.items
    );
    let rendered = result.render(common::SNAPSHOT_STYLE).join("\n");
    assert!(!rendered.contains("sleepy"), "got {rendered}");
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(&yaml, &rendered));
}

#[test]
fn enum_values_within_set_ok() {
    let result = check_column(
        "REQUIRED BYTE_ARRAY status (UTF8)",
        write_strings(&["active", "banned", "active"]),
        indoc! {"
            - name: status
              type: enum
              values: [active, banned]
        "},
    );

    assert_eq!(result.status(), Status::Ok, "got {:?}", result.items);
}

#[test]
fn enum_map_form_values_are_the_keys() {
    // The map form's keys are the allowed values; the labels are ignored.
    let result = check_column(
        "REQUIRED BYTE_ARRAY status (UTF8)",
        write_strings(&["A", "Active"]),
        indoc! {"
            - name: status
              type: enum
              values:
                A: Active
                B: Banned
        "},
    );

    assert_eq!(result.status(), Status::Error);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                kind: ProblemKind::ValuesOutsideEnum { count: 1, rows, values },
                ..
            }] if rows == &[2] && values == &["Active"]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn nulls_in_optional_enum_are_not_outside_values() {
    // A null is the concern of D01 (and only when required); it is never an
    // "outside the set" value.
    let result = check_column(
        "OPTIONAL BYTE_ARRAY status (UTF8)",
        |col| {
            let bytes = [ByteArray::from("active"), ByteArray::from("banned")];
            col.typed::<ByteArrayType>()
                .write_batch(&bytes, Some(&[1, 0, 1]), None)
                .unwrap();
        },
        indoc! {"
            - name: status
              type: enum
              values: [active, banned]
        "},
    );

    assert_eq!(result.status(), Status::Ok, "got {:?}", result.items);
}

#[test]
fn true_parquet_enum_column_is_checked() {
    // A column with the parquet ENUM logical type decodes as binary, not
    // strings; membership must still compare its UTF-8 values.
    let result = check_column(
        "REQUIRED BYTE_ARRAY status (ENUM)",
        |col| {
            col.typed::<ByteArrayType>()
                .write_batch(
                    &[
                        ByteArray::from("active"),
                        ByteArray::from("other"),
                        ByteArray::from("banned"),
                    ],
                    None,
                    None,
                )
                .unwrap();
        },
        indoc! {"
            - name: status
              type: enum
              values: [active, banned]
        "},
    );

    assert_eq!(result.status(), Status::Error);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D04"),
                kind: ProblemKind::ValuesOutsideEnum { count: 1, rows, values },
                ..
            }] if rows == &[2] && values == &["other"]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn foreign_key_between_enum_and_string_ok() {
    // A parquet ENUM child (decoded as binary) referencing a plain string
    // parent compares by UTF-8 bytes, so equal values match.
    let dir = temp_dir();
    write_single_column(
        &dir.join("item.parquet"),
        "REQUIRED BYTE_ARRAY category_id (ENUM)",
        |col| {
            col.typed::<ByteArrayType>()
                .write_batch(&[ByteArray::from("a"), ByteArray::from("b")], None, None)
                .unwrap();
        },
    );
    write_single_column(
        &dir.join("category.parquet"),
        "REQUIRED BYTE_ARRAY id (STRING)",
        |col| {
            col.typed::<ByteArrayType>()
                .write_batch(
                    &[
                        ByteArray::from("a"),
                        ByteArray::from("b"),
                        ByteArray::from("c"),
                    ],
                    None,
                    None,
                )
                .unwrap();
        },
    );
    let yaml = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: item
                source:
                  parquet: item.parquet
                columns:
                  - name: category_id
                    type: enum
                    constraints: [foreign_key]
                    values: [a, b]
              - name: category
                source:
                  parquet: category.parquet
                columns:
                  - name: id
                    type: string
                    constraints: [primary_key]
                    examples: [a, b, c]
            relationships:
              - join: item.category_id = category.id
                cardinality: many-to-one
        "},
    );
    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Ok, "got {:?}", result.items);
}

#[test]
fn enum_over_numeric_column_is_type_mismatch() {
    // An enum's underlying column must be string-like; a numeric backing is an
    // M01, and its values are not scanned for membership (no D04 alongside).
    let result = check_column(
        "REQUIRED INT32 grade",
        |col| {
            col.typed::<Int32Type>()
                .write_batch(&[1, 2, 3], None, None)
                .unwrap();
        },
        indoc! {"
            - name: grade
              type: enum
              values: ['1', '2']
        "},
    );

    assert_eq!(result.status(), Status::Error);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("M01"),
                kind: ProblemKind::TypeMismatch { .. },
                ..
            }]
        ),
        "got {:?}",
        result.items
    );
}

/// With dictionary encoding disabled, the D04 dictionary fast-path can't prove
/// conformance and must fall back to the value scan — which still finds the
/// violation and its exact row.
#[test]
fn enum_without_dictionary_encoding_falls_back_to_scan() {
    let no_dict = || {
        WriterProperties::builder()
            .set_dictionary_enabled(false)
            .build()
    };

    let clean = build_column_with_properties(
        "REQUIRED BYTE_ARRAY status (UTF8)",
        write_strings(&["active", "banned", "active"]),
        indoc! {"
            - name: status
              type: enum
              values: [active, banned]
        "},
        no_dict(),
    );
    assert_eq!(validate_data(&clean, None).status(), Status::Ok);

    let bad = build_column_with_properties(
        "REQUIRED BYTE_ARRAY status (UTF8)",
        write_strings(&["active", "banned", "sleepy"]),
        indoc! {"
            - name: status
              type: enum
              values: [active, banned]
        "},
        no_dict(),
    );
    let result = validate_data(&bad, None);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D04"),
                kind: ProblemKind::ValuesOutsideEnum { count: 1, rows, values },
                ..
            }] if rows == &[3] && values == &["sleepy"]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn primary_key_implies_required_for_nulls() {
    // `primary_key` implies `required`, so the null is reported even without an
    // explicit `required` constraint.
    let result = check_column(
        "OPTIONAL DOUBLE weight",
        write_double_with_null,
        indoc! {"
            - name: weight
              type: number(id)
              constraints: [primary_key]
              examples: [1, 2]
        "},
    );

    assert_eq!(result.status(), Status::Error);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                kind: ProblemKind::NullsInRequired { .. },
                ..
            }]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn duplicate_values_in_unique_column_reported() {
    let result = check_column(
        "REQUIRED DOUBLE id",
        |col| {
            col.typed::<DoubleType>()
                .write_batch(&[1.0, 1.0, 2.0], None, None)
                .unwrap();
        },
        indoc! {"
            - name: id
              type: number(id)
              constraints: [unique]
              examples: [1, 2]
        "},
    );

    assert!(matches!(
        result.items.as_slice(),
        [Problem {
            code: Some("D02"),
            kind: ProblemKind::DuplicateValues { columns, count: 1, rows },
            ..
        }] if columns == &["id"] && rows == &[2]
    ));
}

/// Write a single required string column whose values are split across the
/// given row groups, so the scan accumulates row offsets across group
/// boundaries and exercises the variable-length byte-key path.
fn build_string_groups(groups: &[&[&str]]) -> PathBuf {
    let dir = temp_dir();
    let parquet = dir.join("data.parquet");
    let schema = Arc::new(
        parse_message_type("message schema { REQUIRED BYTE_ARRAY code (UTF8); }").unwrap(),
    );
    let file = File::create(&parquet).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build()))
            .unwrap();
    for group in groups {
        let values = group
            .iter()
            .map(|s| ByteArray::from(*s))
            .collect::<Vec<_>>();
        let mut row_group = writer.next_row_group().unwrap();
        let mut col = row_group.next_column().unwrap().unwrap();
        col.typed::<ByteArrayType>()
            .write_batch(&values, None, None)
            .unwrap();
        col.close().unwrap();
        row_group.close().unwrap();
    }
    writer.close().unwrap();

    write_dict(
        &dir,
        indoc! {"
            tables:
              - name: t
                source:
                  parquet: data.parquet
                columns:
                  - name: code
                    type: string
                    constraints: [unique]
                    examples: [a, b]
        "},
    )
}

#[test]
fn duplicate_string_values_across_row_groups_reported() {
    // No duplicates across two groups.
    let unique = build_string_groups(&[&["a", "b"], &["c", "d"]]);
    assert_eq!(validate_data(&unique, None).status(), Status::Ok);

    // "a" recurs in the second group, so the duplicate sits at row 4 — proving
    // row numbers carry across the row-group boundary.
    let duplicate = build_string_groups(&[&["a", "b"], &["c", "a"]]);
    let result = validate_data(&duplicate, None);
    assert!(matches!(
        result.items.as_slice(),
        [Problem {
            code: Some("D02"),
            kind: ProblemKind::DuplicateValues { columns, count: 1, rows },
            ..
        }] if columns == &["code"] && rows == &[4]
    ));
}

#[test]
fn composite_primary_key_is_checked_collectively() {
    let unique = build_composite_key(&[1.0, 1.0, 2.0], &[1.0, 2.0, 1.0]);
    assert_eq!(validate_data(&unique, None).status(), Status::Ok);

    let duplicate = build_composite_key(&[1.0, 1.0, 2.0], &[1.0, 1.0, 2.0]);
    let result = validate_data(&duplicate, None);
    assert!(matches!(
        result.items.as_slice(),
        [Problem {
            code: Some("D02"),
            kind: ProblemKind::DuplicateValues { columns, count: 1, rows },
            ..
        }] if columns == &["a", "b"] && rows == &[2]
    ));
}

#[test]
fn nulls_in_unique_column_are_not_duplicates() {
    // Rows (1-based): 1 = 1.0, 2 = null, 3 = null, 4 = 2.0. Nulls are exempt from
    // uniqueness, so repeated nulls alongside distinct values are fine.
    let result = check_column(
        "OPTIONAL DOUBLE id",
        |col| {
            col.typed::<DoubleType>()
                .write_batch(&[1.0, 2.0], Some(&[1, 0, 0, 1]), None)
                .unwrap();
        },
        indoc! {"
            - name: id
              type: number(id)
              constraints: [unique]
              examples: [1, 2]
        "},
    );

    assert_eq!(result.status(), Status::Ok, "got {:?}", result.items);
}

#[test]
fn nulls_alongside_a_real_duplicate_report_only_the_duplicate() {
    // Rows (1-based): 1 = 1.0, 2 = null, 3 = 1.0, 4 = null. The nulls are exempt;
    // only the genuine repeat of 1.0 at row 3 is a duplicate.
    let result = check_column(
        "OPTIONAL DOUBLE id",
        |col| {
            col.typed::<DoubleType>()
                .write_batch(&[1.0, 1.0], Some(&[1, 0, 1, 0]), None)
                .unwrap();
        },
        indoc! {"
            - name: id
              type: number(id)
              constraints: [unique]
              examples: [1, 2]
        "},
    );

    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D02"),
                kind: ProblemKind::DuplicateValues { columns, count: 1, rows },
                ..
            }] if columns == &["id"] && rows == &[3]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn nulls_in_unique_string_column_are_not_duplicates() {
    // Exercises the single-byte-column path: two nulls, one value, no duplicate.
    let result = check_column(
        "OPTIONAL BYTE_ARRAY code (UTF8)",
        |col| {
            col.typed::<ByteArrayType>()
                .write_batch(&[ByteArray::from("a")], Some(&[1, 0, 0]), None)
                .unwrap();
        },
        indoc! {"
            - name: code
              type: string
              constraints: [unique]
              examples: [a, b]
        "},
    );

    assert_eq!(result.status(), Status::Ok, "got {:?}", result.items);
}

#[test]
fn nulls_in_primary_key_are_not_reported_as_duplicates() {
    // A PK with nulls fails D01 (primary_key implies required); D02 must not
    // additionally flag the repeated nulls as duplicates. Rows: 1 = 1.0,
    // 2 = null, 3 = 2.0, 4 = null — non-null values distinct, two nulls.
    let result = check_column(
        "OPTIONAL DOUBLE id",
        |col| {
            col.typed::<DoubleType>()
                .write_batch(&[1.0, 2.0], Some(&[1, 0, 1, 0]), None)
                .unwrap();
        },
        indoc! {"
            - name: id
              type: number(id)
              constraints: [primary_key]
              examples: [1, 2]
        "},
    );

    assert!(
        result.items.iter().any(|p| p.code == Some("D01")),
        "expected a D01, got {:?}",
        result.items
    );
    assert!(
        result.items.iter().all(|p| p.code != Some("D02")),
        "expected no D02, got {:?}",
        result.items
    );
}

/// Write a two-column parquet with a required `a` and an optional `b` (whose
/// nulls follow `b_def`), both tagged `primary_key`, so a null in `b` exercises
/// the composite-key null path.
fn build_composite_key_optional_b(a: &[f64], b: &[f64], b_def: &[i16]) -> PathBuf {
    let dir = temp_dir();
    let parquet = dir.join("data.parquet");
    let schema = Arc::new(
        parse_message_type("message schema { REQUIRED DOUBLE a; OPTIONAL DOUBLE b; }").unwrap(),
    );
    let file = File::create(&parquet).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build()))
            .unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    let mut col_a = row_group.next_column().unwrap().unwrap();
    col_a
        .typed::<DoubleType>()
        .write_batch(a, None, None)
        .unwrap();
    col_a.close().unwrap();
    let mut col_b = row_group.next_column().unwrap().unwrap();
    col_b
        .typed::<DoubleType>()
        .write_batch(b, Some(b_def), None)
        .unwrap();
    col_b.close().unwrap();
    row_group.close().unwrap();
    writer.close().unwrap();

    write_dict(
        &dir,
        indoc! {"
            tables:
              - name: t
                source:
                  parquet: data.parquet
                columns:
                  - name: a
                    type: number(id)
                    constraints: [primary_key]
                    examples: [1, 2]
                  - name: b
                    type: number(id)
                    constraints: [primary_key]
                    examples: [1, 2]
        "},
    )
}

#[test]
fn nulls_in_composite_primary_key_are_not_reported_as_duplicates() {
    // Rows: (1, 1.0), (2, null), (3, null). The two rows with a null in `b` fail
    // D01, but must not be reported as a D02 duplicate of each other.
    let result = validate_data(
        &build_composite_key_optional_b(&[1.0, 2.0, 3.0], &[1.0], &[1, 0, 0]),
        None,
    );

    assert!(
        result.items.iter().any(|p| p.code == Some("D01")),
        "expected a D01, got {:?}",
        result.items
    );
    assert!(
        result.items.iter().all(|p| p.code != Some("D02")),
        "expected no D02, got {:?}",
        result.items
    );
}

/// Statistics disabled so the footer can't settle uniqueness — forcing the value
/// scan, where physical comparison happens and normalization matters.
fn scanned_column(
    schema_col: &str,
    write: impl FnOnce(&mut SerializedColumnWriter),
    column: &str,
) -> PathBuf {
    build_column_with_properties(
        schema_col,
        write,
        column,
        WriterProperties::builder()
            .set_statistics_enabled(EnabledStatistics::None)
            .build(),
    )
}

#[test]
fn json_unique_column_skipped_with_warning() {
    // Two JSON values that are logically equal but differ byte-wise. Comparing
    // physically would flag them as duplicates, so the check is skipped (D03)
    // rather than risk an unsound verdict.
    let yaml = build_column(
        "REQUIRED BYTE_ARRAY notes (JSON)",
        |col| {
            col.typed::<ByteArrayType>()
                .write_batch(
                    &[
                        ByteArray::from(r#"{"a":1}"#),
                        ByteArray::from(r#"{"a": 1}"#),
                    ],
                    None,
                    None,
                )
                .unwrap();
        },
        indoc! {r#"
            - name: notes
              type: string
              constraints: [unique]
              examples: ["{}"]
        "#},
    );

    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Warning, "got {:?}", result.items);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D03"),
                kind: ProblemKind::UniquenessNotVerified { columns, reason },
                ..
            }] if columns == &["notes"] && reason == "json"
        ),
        "got {:?}",
        result.items
    );
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(
        &yaml,
        &result.render(common::SNAPSHOT_STYLE).join("\n")
    ));
}

#[test]
fn json_in_primary_key_skips_whole_key_with_warning() {
    let dir = temp_dir();
    let parquet = dir.join("data.parquet");
    let schema = Arc::new(
        parse_message_type(
            "message schema { REQUIRED INT64 id; REQUIRED BYTE_ARRAY payload (JSON); }",
        )
        .unwrap(),
    );
    let file = File::create(&parquet).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build()))
            .unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    let mut id = row_group.next_column().unwrap().unwrap();
    id.typed::<Int64Type>()
        .write_batch(&[1, 2, 3], None, None)
        .unwrap();
    id.close().unwrap();
    let mut payload = row_group.next_column().unwrap().unwrap();
    payload
        .typed::<ByteArrayType>()
        .write_batch(
            &[
                ByteArray::from(r#"{"x":1}"#),
                ByteArray::from(r#"{"x":2}"#),
                ByteArray::from(r#"{"x":3}"#),
            ],
            None,
            None,
        )
        .unwrap();
    payload.close().unwrap();
    row_group.close().unwrap();
    writer.close().unwrap();

    let yaml = write_dict(
        &dir,
        indoc! {r#"
            tables:
              - name: t
                source:
                  parquet: data.parquet
                columns:
                  - name: id
                    type: number(id)
                    constraints: [primary_key]
                    examples: [1, 2]
                  - name: payload
                    type: string
                    constraints: [primary_key]
                    examples: ["{}"]
        "#},
    );

    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Warning, "got {:?}", result.items);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D03"),
                message,
                kind: ProblemKind::UniquenessNotVerified { columns, reason },
                ..
            }] if columns == &["id", "payload"] && reason == "json" && message.contains("payload")
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn differently_encoded_decimals_are_duplicates() {
    // Unscaled 1 encoded as `01` and as `00 01`: logically equal, so after
    // normalization the second row is a duplicate.
    let yaml = scanned_column(
        "REQUIRED BYTE_ARRAY amount (DECIMAL(9,2))",
        |col| {
            col.typed::<ByteArrayType>()
                .write_batch(
                    &[
                        ByteArray::from(vec![0x01_u8]),
                        ByteArray::from(vec![0x00_u8, 0x01]),
                        ByteArray::from(vec![0x02_u8]),
                    ],
                    None,
                    None,
                )
                .unwrap();
        },
        indoc! {"
            - name: amount
              type: number(id)
              constraints: [unique]
              examples: [1, 2]
        "},
    );

    let result = validate_data(&yaml, None);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D02"),
                kind: ProblemKind::DuplicateValues { columns, count: 1, rows },
                ..
            }] if columns == &["amount"] && rows == &[2]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn signed_zeros_are_duplicates() {
    // `-0.0` and `+0.0` collapse to one value, so the second is a duplicate.
    let yaml = scanned_column(
        "REQUIRED DOUBLE score",
        |col| {
            col.typed::<DoubleType>()
                .write_batch(&[0.0, -0.0, 3.0], None, None)
                .unwrap();
        },
        indoc! {"
            - name: score
              type: number(id)
              constraints: [unique]
              examples: [1, 2]
        "},
    );

    let result = validate_data(&yaml, None);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D02"),
                kind: ProblemKind::DuplicateValues { columns, count: 1, rows },
                ..
            }] if columns == &["score"] && rows == &[2]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn float16_unique_column_is_checked() {
    // 16-bit floats are comparable (with the same signed-zero collapsing), so
    // uniqueness is verified rather than skipped with a D03.
    let zero = FixedLenByteArray::from(vec![0x00_u8, 0x00]); // +0.0
    let negative_zero = FixedLenByteArray::from(vec![0x00_u8, 0x80]); // -0.0
    let one_and_a_half = FixedLenByteArray::from(vec![0x00_u8, 0x3E]); // 1.5
    let yaml = scanned_column(
        "REQUIRED FIXED_LEN_BYTE_ARRAY(2) reading (FLOAT16)",
        |col| {
            col.typed::<FixedLenByteArrayType>()
                .write_batch(&[zero, negative_zero, one_and_a_half], None, None)
                .unwrap();
        },
        indoc! {"
            - name: reading
              type: number(id)
              constraints: [unique]
              examples: [1.5]
        "},
    );

    let result = validate_data(&yaml, None);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D02"),
                kind: ProblemKind::DuplicateValues { columns, count: 1, rows },
                ..
            }] if columns == &["reading"] && rows == &[2]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn distinct_nan_bit_patterns_are_duplicates() {
    // Two different NaN encodings collapse to one value, so the second is a
    // duplicate of the first.
    let nan1 = f64::from_bits(0x7ff8_0000_0000_0001);
    let nan2 = f64::from_bits(0x7ff8_0000_0000_0002);
    let yaml = scanned_column(
        "REQUIRED DOUBLE score",
        |col| {
            col.typed::<DoubleType>()
                .write_batch(&[nan1, nan2, 3.0], None, None)
                .unwrap();
        },
        indoc! {"
            - name: score
              type: number(id)
              constraints: [unique]
              examples: [1, 2]
        "},
    );

    let result = validate_data(&yaml, None);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D02"),
                kind: ProblemKind::DuplicateValues { columns, count: 1, rows },
                ..
            }] if columns == &["score"] && rows == &[2]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn int_backed_decimal_unique_column_passes() {
    // Int-backed decimals are canonical, so distinct unscaled values are clean.
    let yaml = scanned_column(
        "REQUIRED INT64 amount (DECIMAL(9,2))",
        |col| {
            col.typed::<Int64Type>()
                .write_batch(&[100, 200, 300], None, None)
                .unwrap();
        },
        indoc! {"
            - name: amount
              type: number(id)
              constraints: [unique]
              examples: [1, 2]
        "},
    );

    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

// --- Foreign keys (D05/D06) -------------------------------------------------

/// Write a single-column parquet file at `path`.
fn write_single_column(
    path: &Path,
    schema_col: &str,
    write: impl FnOnce(&mut SerializedColumnWriter),
) {
    let message = format!("message schema {{ {schema_col}; }}");
    let schema = Arc::new(parse_message_type(&message).unwrap());
    let file = File::create(path).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build()))
            .unwrap();
    let mut rg = writer.next_row_group().unwrap();
    let mut col = rg.next_column().unwrap().unwrap();
    write(&mut col);
    col.close().unwrap();
    rg.close().unwrap();
    writer.close().unwrap();
}

/// Build a two-table dictionary with a foreign key `item.category_id` →
/// `category.id`, backed by two single-column parquet files. `col_type` and
/// `examples` supply the dictionary type and representation for both columns.
fn build_fk(
    child_schema: &str,
    child_write: impl FnOnce(&mut SerializedColumnWriter),
    parent_schema: &str,
    parent_write: impl FnOnce(&mut SerializedColumnWriter),
    col_type: &str,
    examples: &str,
) -> PathBuf {
    let dir = temp_dir();
    write_single_column(&dir.join("item.parquet"), child_schema, child_write);
    write_single_column(&dir.join("category.parquet"), parent_schema, parent_write);
    write_dict(
        &dir,
        &formatdoc! {"
            tables:
              - name: item
                source:
                  parquet: item.parquet
                columns:
                  - name: category_id
                    type: {col_type}
                    constraints: [foreign_key]
                    examples: {examples}
              - name: category
                source:
                  parquet: category.parquet
                columns:
                  - name: id
                    type: {col_type}
                    constraints: [primary_key]
                    examples: {examples}
            relationships:
              - join: item.category_id = category.id
                cardinality: many-to-one
        "},
    )
}

#[test]
fn foreign_key_orphan_value_reported() {
    // Child id 5 (row 3) has no matching primary key in the parent.
    let yaml = build_fk(
        "REQUIRED INT64 category_id",
        |col| {
            col.typed::<Int64Type>()
                .write_batch(&[1, 2, 5], None, None)
                .unwrap();
        },
        "REQUIRED INT64 id",
        |col| {
            col.typed::<Int64Type>()
                .write_batch(&[1, 2, 3], None, None)
                .unwrap();
        },
        "number(id)",
        "[1, 2]",
    );
    let result = validate_data(&yaml, None);

    assert_eq!(result.status(), Status::Error, "got {:?}", result.items);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D05"),
                kind: ProblemKind::ForeignKeyNotFound { column, references, count: 1, rows, values },
                ..
            }] if column == "category_id"
                && references == "category.id"
                && rows == &[3]
                && values == &["5"]
        ),
        "got {:?}",
        result.items
    );
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(
        &yaml,
        &result.render(common::SNAPSHOT_STYLE).join("\n")
    ));
}

/// A restricted foreign key's orphan values are withheld from both the
/// message and the structured kind.
#[test]
fn foreign_key_orphans_withheld_when_restricted() {
    let dir = temp_dir();
    write_single_column(
        &dir.join("item.parquet"),
        "REQUIRED INT64 category_id",
        |col| {
            col.typed::<Int64Type>()
                .write_batch(&[1, 2, 5], None, None)
                .unwrap();
        },
    );
    write_single_column(&dir.join("category.parquet"), "REQUIRED INT64 id", |col| {
        col.typed::<Int64Type>()
            .write_batch(&[1, 2, 3], None, None)
            .unwrap();
    });
    let yaml = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: item
                source:
                  parquet: item.parquet
                columns:
                  - name: category_id
                    type: number(id)
                    constraints: [foreign_key]
                    display: restricted
                    examples: [1, 2]
              - name: category
                source:
                  parquet: category.parquet
                columns:
                  - name: id
                    type: number(id)
                    constraints: [primary_key]
                    examples: [1, 2]
            relationships:
              - join: item.category_id = category.id
                cardinality: many-to-one
        "},
    );
    let result = validate_data(&yaml, None);

    assert_eq!(result.status(), Status::Error, "got {:?}", result.items);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D05"),
                kind: ProblemKind::ForeignKeyNotFound { count: 1, rows, values, .. },
                ..
            }] if rows == &[3] && values.is_empty()
        ),
        "got {:?}",
        result.items
    );
    let rendered = result.render(common::SNAPSHOT_STYLE).join("\n");
    assert!(!rendered.contains("`5`"), "got {rendered}");
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(&yaml, &rendered));
}

#[test]
fn foreign_key_all_values_present_ok() {
    let yaml = build_fk(
        "REQUIRED INT64 category_id",
        |col| {
            col.typed::<Int64Type>()
                .write_batch(&[1, 2, 1], None, None)
                .unwrap();
        },
        "REQUIRED INT64 id",
        |col| {
            col.typed::<Int64Type>()
                .write_batch(&[1, 2, 3], None, None)
                .unwrap();
        },
        "number(id)",
        "[1, 2]",
    );
    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Ok, "got {:?}", result.items);
}

#[test]
fn foreign_key_null_values_are_exempt() {
    // Rows: 1 = 1, 2 = null, 3 = 5. The null references nothing (exempt); only
    // the orphan 5 at row 3 is reported, proving null rows are skipped and row
    // numbering still counts them.
    let yaml = build_fk(
        "OPTIONAL INT64 category_id",
        |col| {
            col.typed::<Int64Type>()
                .write_batch(&[1, 5], Some(&[1, 0, 1]), None)
                .unwrap();
        },
        "REQUIRED INT64 id",
        |col| {
            col.typed::<Int64Type>()
                .write_batch(&[1, 2, 3], None, None)
                .unwrap();
        },
        "number(id)",
        "[1, 2]",
    );
    let result = validate_data(&yaml, None);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D05"),
                kind: ProblemKind::ForeignKeyNotFound { count: 1, rows, values, .. },
                ..
            }] if rows == &[3] && values == &["5"]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn foreign_key_string_orphan_reported() {
    let yaml = build_fk(
        "REQUIRED BYTE_ARRAY category_id (UTF8)",
        write_strings(&["a", "b", "z"]),
        "REQUIRED BYTE_ARRAY id (UTF8)",
        write_strings(&["a", "b", "c"]),
        "string",
        "[a, b]",
    );
    let result = validate_data(&yaml, None);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D05"),
                kind: ProblemKind::ForeignKeyNotFound { references, count: 1, rows, values, .. },
                ..
            }] if references == "category.id" && rows == &[3] && values == &["z"]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn foreign_key_across_int_widths_ok() {
    // An INT32 child against an INT64 parent: both cast to a common i64, so
    // equal ids match despite the differing physical width.
    let yaml = build_fk(
        "REQUIRED INT32 category_id",
        |col| {
            col.typed::<Int32Type>()
                .write_batch(&[1, 2], None, None)
                .unwrap();
        },
        "REQUIRED INT64 id",
        |col| {
            col.typed::<Int64Type>()
                .write_batch(&[1, 2, 3], None, None)
                .unwrap();
        },
        "number(id)",
        "[1, 2]",
    );
    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Ok, "got {:?}", result.items);
}

#[test]
fn foreign_key_across_decimal_encodings() {
    // An int-backed DECIMAL(9,2) child against a byte-backed DECIMAL(18,2)
    // parent: values are compared numerically, so unscaled 100 matches the
    // parent's `0x64` and only the genuinely-absent 9.99 is an orphan —
    // rendered at the column's scale.
    let yaml = build_fk(
        "REQUIRED INT32 category_id (DECIMAL(9,2))",
        |col| {
            col.typed::<Int32Type>()
                .write_batch(&[100, 999], None, None)
                .unwrap();
        },
        "REQUIRED BYTE_ARRAY id (DECIMAL(18,2))",
        |col| {
            col.typed::<ByteArrayType>()
                .write_batch(
                    &[
                        ByteArray::from(vec![0x64_u8]),
                        ByteArray::from(vec![0x00_u8, 0xFA]),
                    ],
                    None,
                    None,
                )
                .unwrap();
        },
        "number(id)",
        "[1, 2]",
    );
    let result = validate_data(&yaml, None);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D05"),
                kind: ProblemKind::ForeignKeyNotFound { count: 1, rows, values, .. },
                ..
            }] if rows == &[2] && values == &["9.99"]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn foreign_key_without_common_form_reports_all() {
    // A string child referencing a numeric parent has no common comparable
    // form: nothing can match, so every non-null child value is an orphan
    // (the null at row 2 stays exempt).
    let dir = temp_dir();
    write_single_column(
        &dir.join("item.parquet"),
        "OPTIONAL BYTE_ARRAY category_id (STRING)",
        |col| {
            col.typed::<ByteArrayType>()
                .write_batch(
                    &[ByteArray::from("a"), ByteArray::from("b")],
                    Some(&[1, 0, 1]),
                    None,
                )
                .unwrap();
        },
    );
    write_single_column(&dir.join("category.parquet"), "REQUIRED INT64 id", |col| {
        col.typed::<Int64Type>()
            .write_batch(&[1, 2], None, None)
            .unwrap();
    });
    let yaml = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: item
                source:
                  parquet: item.parquet
                columns:
                  - name: category_id
                    type: string
                    constraints: [foreign_key]
                    examples: [a, b]
              - name: category
                source:
                  parquet: category.parquet
                columns:
                  - name: id
                    type: number(id)
                    constraints: [primary_key]
                    examples: [1, 2]
            relationships:
              - join: item.category_id = category.id
                cardinality: many-to-one
        "},
    );
    let result = validate_data(&yaml, None);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D05"),
                kind: ProblemKind::ForeignKeyNotFound { count: 2, rows, values, .. },
                ..
            }] if rows == &[1, 3] && values == &["a", "b"]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn foreign_key_date_orphan_renders_as_date() {
    // Orphan samples are rendered at the column's logical type: a DATE value
    // reads as `2024-01-02`, not its raw day count.
    let dir = temp_dir();
    write_single_column(
        &dir.join("item.parquet"),
        "REQUIRED INT32 seen_on (DATE)",
        |col| {
            col.typed::<Int32Type>()
                .write_batch(&[19723, 19724], None, None)
                .unwrap();
        },
    );
    write_single_column(
        &dir.join("category.parquet"),
        "REQUIRED INT32 held_on (DATE)",
        |col| {
            col.typed::<Int32Type>()
                .write_batch(&[19723], None, None)
                .unwrap();
        },
    );
    let yaml = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: item
                source:
                  parquet: item.parquet
                columns:
                  - name: seen_on
                    type: date
                    constraints: [foreign_key]
                    range: [2024-01-01, 2024-01-02]
              - name: category
                source:
                  parquet: category.parquet
                columns:
                  - name: held_on
                    type: date
                    constraints: [primary_key]
                    range: [2024-01-01, 2024-01-01]
            relationships:
              - join: item.seen_on = category.held_on
                cardinality: many-to-one
        "},
    );
    let result = validate_data(&yaml, None);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D05"),
                kind: ProblemKind::ForeignKeyNotFound { count: 1, rows, values, .. },
                ..
            }] if rows == &[2] && values == &["2024-01-02"]
        ),
        "got {:?}",
        result.items
    );
}

#[test]
fn foreign_key_incomparable_type_not_verified() {
    // The foreign-key column is JSON, whose values can't be compared, so the
    // reference is reported as unverified (D06) rather than checked.
    let yaml = build_fk(
        "REQUIRED BYTE_ARRAY category_id (JSON)",
        write_strings(&[r#"{"a":1}"#]),
        "REQUIRED BYTE_ARRAY id (UTF8)",
        write_strings(&["x"]),
        "string",
        r#"["{}"]"#,
    );
    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Warning, "got {:?}", result.items);
    assert!(
        matches!(
            result.items.as_slice(),
            [Problem {
                code: Some("D06"),
                kind: ProblemKind::ReferentialIntegrityNotVerified { column, references, reason },
                ..
            }] if column == "category_id" && references == "category.id" && reason == "json"
        ),
        "got {:?}",
        result.items
    );
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(
        &yaml,
        &result.render(common::SNAPSHOT_STYLE).join("\n")
    ));
}

// --- nested columns (struct fields and lists) -------------------------------

/// Build a dictionary over the nested fixture (see `write_nested_parquet`)
/// with the given `columns:` entries, returning the dictionary path.
fn nested_dict(columns: &str) -> PathBuf {
    let dir = temp_dir();
    common::write_nested_parquet(&dir.join("data.parquet"));
    let columns = columns
        .trim_end()
        .lines()
        .map(|line| format!("      {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    write_dict(
        &dir,
        &formatdoc! {"
            tables:
              - name: animals
                source:
                  parquet: data.parquet
                columns:
                  - name: name
                    type: string
                    examples: [otter, seal]
            {columns}
        "},
    )
}

// D01: `required` on a nested column is about the container itself — the null
// struct and null list on row 3 each count once.
#[test]
fn null_containers_violate_required() {
    let yaml = nested_dict(indoc! {"
        - name: addr
          type: struct
          constraints: [required]
          fields:
            - name: zip
              type: string
              examples: ['97201']
            - name: country
              type: enum
              values: [US, CA, XX]
        - name: tags
          type: list(enum)
          constraints: [required]
          values: [a, b, zz]
    "});

    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Error);
    let d01s: Vec<_> = result
        .items
        .iter()
        .filter(|p| p.code == Some("D01"))
        .collect();
    assert_eq!(d01s.len(), 2, "got {:?}", result.items);
    assert!(
        d01s.iter().all(|p| matches!(
            &p.kind,
            ProblemKind::NullsInRequired { count: 1, rows } if rows == &vec![3]
        )),
        "got {:?}",
        result.items
    );
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(
        &yaml,
        &result.render(common::SNAPSHOT_STYLE).join("\n")
    ));
}

// D04 checks the elements of a `list(enum)`: row 2 holds the undeclared `zz`.
#[test]
fn list_enum_membership_checked() {
    let yaml = nested_dict(indoc! {"
        - name: addr
          type: struct
          fields:
            - name: zip
              type: string
              examples: ['97201']
            - name: country
              type: enum
              values: [US, CA, XX]
        - name: tags
          type: list(enum)
          values: [a, b]
    "});

    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Error);
    assert!(
        result.items.iter().any(|p| matches!(
            &p.kind,
            ProblemKind::ValuesOutsideEnum { count: 1, rows, values }
                if rows == &vec![2] && values == &vec!["zz".to_string()]
        )),
        "got {:?}",
        result.items
    );
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(
        &yaml,
        &result.render(common::SNAPSHOT_STYLE).join("\n")
    ));
}

// D04 checks `enum` fields inside a struct: row 2's `addr.country` is `XX`.
#[test]
fn struct_enum_field_membership_checked() {
    let yaml = nested_dict(indoc! {"
        - name: addr
          type: struct
          fields:
            - name: zip
              type: string
              examples: ['97201']
            - name: country
              type: enum
              values: [US, CA]
        - name: tags
          type: list(enum)
          values: [a, b, zz]
    "});

    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Error);
    assert!(
        result.items.iter().any(|p| matches!(
            &p.kind,
            ProblemKind::ValuesOutsideEnum { count: 1, rows, values }
                if rows == &vec![2] && values == &vec!["XX".to_string()]
        )),
        "got {:?}",
        result.items
    );
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(
        &yaml,
        &result.render(common::SNAPSHOT_STYLE).join("\n")
    ));
}

// A nested dictionary whose data conforms raises nothing at the data level.
#[test]
fn conforming_nested_data_is_clean() {
    let yaml = nested_dict(indoc! {"
        - name: addr
          type: struct
          fields:
            - name: zip
              type: string
              examples: ['97201']
            - name: country
              type: enum
              values: [US, CA, XX]
        - name: tags
          type: list(enum)
          values: [a, b, zz]
    "});

    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Ok, "got {:?}", result.items);
}

// D01 and D04 compose through nested lists: the container's null counts as
// missing, and an element two list layers down is attributed to its row.
#[test]
fn nested_list_checks_compose() {
    let dir = temp_dir();
    common::write_matrix_parquet(&dir.join("matrix.parquet"));
    let yaml = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: matrix
                source:
                  parquet: matrix.parquet
                columns:
                  - name: grid
                    type: list(list(enum))
                    constraints: [required]
                    values: [a, b]
        "},
    );

    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Error);
    assert!(
        result.items.iter().any(|p| matches!(
            &p.kind,
            ProblemKind::NullsInRequired { count: 1, rows } if rows == &vec![3]
        )),
        "got {:?}",
        result.items
    );
    assert!(
        result.items.iter().any(|p| matches!(
            &p.kind,
            ProblemKind::ValuesOutsideEnum { count: 1, rows, values }
                if rows == &vec![2] && values == &vec!["zz".to_string()]
        )),
        "got {:?}",
        result.items
    );
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(
        &yaml,
        &result.render(common::SNAPSHOT_STYLE).join("\n")
    ));
}

// The same dictionary is clean once the declared values cover the data.
#[test]
fn conforming_nested_list_data_is_clean() {
    let dir = temp_dir();
    common::write_matrix_parquet(&dir.join("matrix.parquet"));
    let yaml = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: matrix
                source:
                  parquet: matrix.parquet
                columns:
                  - name: grid
                    type: list(list(enum))
                    values: [a, b, zz]
        "},
    );

    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Ok, "got {:?}", result.items);
}

// Alternating nesting — struct → list(struct) → struct → list(enum) — reaches
// the data level too: the container's null trips `required`, and an element
// four levels down is attributed to its row through both list layers.
#[test]
fn deep_alternating_nesting_checks_values() {
    let dir = temp_dir();
    common::write_deep_parquet(&dir.join("data.parquet"));
    let yaml = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: orders
                source:
                  parquet: data.parquet
                columns:
                  - name: order
                    type: struct
                    constraints: [required]
                    fields:
                      - name: shipments
                        type: list(struct)
                        fields:
                          - name: origin
                            type: struct
                            fields:
                              - name: statuses
                                type: list(enum)
                                values: [ok, late]
        "},
    );

    let result = validate_data(&yaml, None);
    assert_eq!(result.status(), Status::Error);
    assert!(
        result.items.iter().any(|p| matches!(
            &p.kind,
            ProblemKind::NullsInRequired { count: 1, rows } if rows == &vec![3]
        )),
        "got {:?}",
        result.items
    );
    assert!(
        result.items.iter().any(|p| matches!(
            &p.kind,
            ProblemKind::ValuesOutsideEnum { count: 1, rows, values }
                if rows == &vec![2] && values == &vec!["bogus".to_string()]
        )),
        "got {:?}",
        result.items
    );
    #[cfg(unix)]
    assert_snapshot!(common::diagnostic(
        &yaml,
        &result.render(common::SNAPSHOT_STYLE).join("\n")
    ));
}

// --- assertions (D07–D09) -------------------------------------------------
//
// An `assert` expression is checked for form at the spec level and evaluated
// here; see `site/expression-execution.md` for what evaluation means.

/// A two-column table of integers, with `constraints` spliced in at table level.
fn build_asserted(a: &[i64], b: &[Option<i64>], constraints: &str) -> PathBuf {
    let dir = temp_dir();
    let parquet = dir.join("data.parquet");
    let schema = Arc::new(
        parse_message_type("message schema { REQUIRED INT64 a; OPTIONAL INT64 b; }").unwrap(),
    );
    let file = File::create(&parquet).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build()))
            .unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    let mut first = row_group.next_column().unwrap().unwrap();
    first
        .typed::<Int64Type>()
        .write_batch(a, None, None)
        .unwrap();
    first.close().unwrap();
    let mut second = row_group.next_column().unwrap().unwrap();
    let levels: Vec<i16> = b.iter().map(|v| v.is_some() as i16).collect();
    let present: Vec<i64> = b.iter().flatten().copied().collect();
    second
        .typed::<Int64Type>()
        .write_batch(&present, Some(&levels), None)
        .unwrap();
    second.close().unwrap();
    row_group.close().unwrap();
    writer.close().unwrap();

    write_dict(
        &dir,
        &formatdoc! {"
            tables:
              - name: t
                source:
                  parquet: data.parquet
                constraints:
            {constraints}
                columns:
                  - name: a
                    type: number(quantity)
                    range: [-1000000, 1000000]
                  - name: b
                    type: number(quantity)
                    range: [-1000000, 1000000]
        "},
    )
}

/// A one-column table of floats named `x`, so an assertion can meet the values
/// only a float can hold: an infinity, a NaN, a negative zero.
fn build_asserted_floats(x: &[f64], constraints: &str) -> PathBuf {
    let dir = temp_dir();
    let parquet = dir.join("data.parquet");
    let schema = Arc::new(parse_message_type("message schema { REQUIRED DOUBLE x; }").unwrap());
    let file = File::create(&parquet).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build()))
            .unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    let mut column = row_group.next_column().unwrap().unwrap();
    column
        .typed::<DoubleType>()
        .write_batch(x, None, None)
        .unwrap();
    column.close().unwrap();
    row_group.close().unwrap();
    writer.close().unwrap();

    write_dict(
        &dir,
        &formatdoc! {"
            tables:
              - name: t
                source:
                  parquet: data.parquet
                constraints:
            {constraints}
                columns:
                  - name: x
                    type: number
                    examples: [1.5, 2.5, 3.5]
        "},
    )
}

fn assertion(text: &str) -> String {
    format!("      - assert: {text}")
}

/// Validate and render, the shape every assertion test below asserts against.
fn asserted(yaml: &Path) -> common::Diagnostic {
    let result = validate_data(yaml, None);
    common::diagnostic(yaml, &result.render(common::SNAPSHOT_STYLE).join("\n"))
}

#[test]
fn assertion_violated_reports_the_rows() {
    let yaml = build_asserted(
        &[1, -2, 3, -4],
        &[None, None, None, None],
        &assertion("a > 0"),
    );
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D07", "is false for 2 rows", "2, 4"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn an_assertion_that_holds_is_silent() {
    let yaml = build_asserted(&[1, 2, 3], &[None, None, None], &assertion("a > 0"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn a_null_operand_passes() {
    // Only `false` is a violation: a comparison against a null is null, which
    // passes, so an assertion is never also a null check.
    let yaml = build_asserted(&[1, 2], &[None, None], &assertion("b > 0"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn dividing_by_zero_yields_an_infinity() {
    // `5 / 0` is `INF`, which is greater than 1, so the row holds.
    let yaml = build_asserted(&[1, 0], &[Some(5), Some(5)], &assertion("b / a > 1"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn a_nan_violates_every_comparison_it_reaches() {
    // `0 / 0` is a NaN, and every comparison against one is false, so the row is
    // reported rather than quietly passing on a null.
    let yaml = build_asserted(&[1, 0], &[Some(5), Some(0)], &assertion("b / a > 1"));
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D07", "is false for 1 row", "(b=0, a=0)"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn a_remainder_takes_its_sign_from_the_divisor() {
    // `MOD(-7, 3)` is 2, not -1: the divisor's sign wins, as in R and Python.
    let yaml = build_asserted(&[-7], &[None], &assertion("MOD(a, 3) = 2"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
    let yaml = build_asserted(&[-7], &[None], &assertion("MOD(a, -3) = -1"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
    // A float divisor takes the same rule.
    let yaml = build_asserted(&[-7], &[None], &assertion("MOD(a, 2.5) = 0.5"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn a_remainder_never_follows_the_dividend() {
    let yaml = build_asserted(&[-7, -6], &[None, None], &assertion("MOD(a, 3) <= 0"));
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D07", "is false for 1 row", "(a=-7)"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn a_zero_modulus_yields_a_nan() {
    // `MOD(5, 0)` is a NaN, so `= 0` is false and the row is reported.
    let yaml = build_asserted(&[1, 0], &[Some(5), Some(5)], &assertion("MOD(b, a) = 0"));
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D07", "is false for 1 row", "(b=5, a=0)"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
    // Tolerating it is `IS_NAN`'s job, not a null's.
    let yaml = build_asserted(
        &[1, 0],
        &[Some(5), Some(5)],
        &assertion("IS_NAN(MOD(b, a)) OR MOD(b, a) = 0"),
    );
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn a_non_finite_value_in_the_data_is_a_value() {
    // Read from the data rather than computed, and neither is null: `IS NULL` is
    // false for both, and the predicates say which is which.
    let data = &[1.5, f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
    let yaml = build_asserted_floats(data, &assertion("x IS NOT NULL"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
    let yaml = build_asserted_floats(
        data,
        &assertion("IS_FINITE(x) OR IS_INFINITE(x) OR IS_NAN(x)"),
    );
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
    let diagnostic = asserted(&build_asserted_floats(data, &assertion("IS_FINITE(x)")));
    diagnostic.assert_contains(&["D07", "is false for 3 rows: 2, 3, 4", "(x=NaN)"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn a_kind_test_propagates_a_null() {
    // `IS NULL` asks whether a value is missing; these ask which kind of number
    // it is, and a null is no kind at all — so the answer is null, and passes.
    let yaml = build_asserted(&[1, 2], &[None, None], &assertion("IS_NAN(b)"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
    let yaml = build_asserted(&[1, 2], &[None, None], &assertion("IS_FINITE(b)"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn a_nan_is_unordered_but_unequal() {
    let nan = &[f64::NAN];
    // Every comparison against a NaN is false — including `NOT (x < 1)`, which
    // is true where `x >= 1` is false.
    for false_assertion in ["x = NAN", "x > 1", "x < 1", "x >= 1", "x BETWEEN 0 AND 1"] {
        let diagnostic = asserted(&build_asserted_floats(nan, &assertion(false_assertion)));
        diagnostic.assert_contains(&["D07", "is false for 1 row"]);
    }
    for true_assertion in [
        "x <> NAN",
        "NOT (x < 1)",
        "x NOT BETWEEN 0 AND 1",
        "IS_NAN(x)",
    ] {
        let yaml = build_asserted_floats(nan, &assertion(true_assertion));
        assert_eq!(
            validate_data(&yaml, None).status(),
            Status::Ok,
            "{true_assertion}"
        );
    }
}

#[test]
fn an_aggregate_folds_a_non_finite_value_in() {
    // `MIN`/`MAX`/`COUNT_DISTINCT` fold on the identity order: a NaN is the
    // largest value, both zeros are one value, and all NaNs are one value.
    let data = &[-0.0, 0.0, 1.5, f64::NAN, f64::NAN, f64::INFINITY];
    for holds in [
        "MAX(x) <> MAX(x)",
        "MIN(x) = 0",
        "COUNT_DISTINCT(x) = 4",
        "COUNT(x) = 6",
        "IS_NAN(SUM(x))",
        "IS_NAN(AVG(x))",
    ] {
        let yaml = build_asserted_floats(data, &assertion(holds));
        assert_eq!(validate_data(&yaml, None).status(), Status::Ok, "{holds}");
    }
    // An infinity alone sums to an infinity, so `SUM` is not simply a NaN.
    let yaml = build_asserted_floats(&[1.0, f64::INFINITY], &assertion("IS_INFINITE(SUM(x))"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn integer_overflow_withdraws_the_verdict() {
    let huge = i64::MAX;
    let yaml = build_asserted(&[1, huge], &[None, None], &assertion("a * 2 > 0"));
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D09", "overflows a 64-bit integer", "row 2"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn an_aggregate_assertion_names_the_table_not_a_row() {
    let yaml = build_asserted(&[1, 2, 3], &[None, None, None], &assertion("SUM(a) > 100"));
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D07", "is false for this table"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn an_aggregate_assertion_that_holds_is_silent() {
    let yaml = build_asserted(&[1, 2, 3], &[None, None, None], &assertion("SUM(a) = 6"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn a_mixed_grain_assertion_is_judged_row_by_row() {
    // `MIN(a)` is folded over the table first, then each row is compared
    // against it — the spec's own example.
    let yaml = build_asserted(
        &[2, 3, 9],
        &[None, None, None],
        &assertion("a <= 2 * MIN(a)"),
    );
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D07", "is false for 1 row", "3"]);

    let holds = build_asserted(
        &[2, 3, 4],
        &[None, None, None],
        &assertion("a <= 2 * MIN(a)"),
    );
    assert_eq!(validate_data(&holds, None).status(), Status::Ok);
}

#[test]
fn counting_aggregates_see_every_row() {
    let yaml = build_asserted(
        &[1, 2, 3, 4],
        &[Some(1), None, Some(3), None],
        &assertion("COUNT(b) >= 0.9 * ROW_COUNT()"),
    );
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D07", "is false for this table"]);
}

#[test]
fn a_filtered_aggregate_folds_only_matching_rows() {
    // Only rows 1 and 3 have `b > 0`, so the sum is 1 + 3.
    let holds = build_asserted(
        &[1, 2, 3],
        &[Some(1), Some(-1), Some(1)],
        &assertion("SUM(a) FILTER (WHERE b > 0) = 4"),
    );
    assert_eq!(validate_data(&holds, None).status(), Status::Ok);

    let fails = build_asserted(
        &[1, 2, 3],
        &[Some(1), Some(-1), Some(1)],
        &assertion("SUM(a) FILTER (WHERE b > 0) = 6"),
    );
    let diagnostic = asserted(&fails);
    diagnostic.assert_contains(&["D07", "is false for this table"]);
}

#[test]
fn a_null_filter_condition_excludes_the_row() {
    // Row 2's condition is null, which excludes it exactly as `false` does,
    // so only row 1 is counted.
    let yaml = build_asserted(
        &[1, 2, 3],
        &[Some(1), None, Some(-1)],
        &assertion("COUNT(a) FILTER (WHERE b > 0) = 1"),
    );
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn a_filter_that_admits_no_rows_leaves_empty_input() {
    // SUM of nothing is null, which passes; COUNT of nothing is 0.
    let yaml = build_asserted(
        &[1, 2],
        &[Some(1), Some(1)],
        &assertion("SUM(a) FILTER (WHERE b > 5) > 100"),
    );
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
    let yaml = build_asserted(
        &[1, 2],
        &[Some(1), Some(1)],
        &assertion("COUNT(a) FILTER (WHERE b > 5) = 0"),
    );
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn a_filtered_row_count_counts_only_matching_rows() {
    let yaml = build_asserted(
        &[1, 2, 3],
        &[Some(1), Some(-1), Some(1)],
        &assertion("ROW_COUNT() FILTER (WHERE b > 0) = 2"),
    );
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn a_filtered_aggregate_folds_before_a_row_assertion_is_judged() {
    // Mixed grain: the filtered sum is folded once (1 + 3 = 4), then compared
    // against each row.
    let holds = build_asserted(
        &[1, 2, 3],
        &[Some(1), Some(-1), Some(1)],
        &assertion("a <= 2 * SUM(a) FILTER (WHERE b > 0)"),
    );
    assert_eq!(validate_data(&holds, None).status(), Status::Ok);

    // The large value is excluded from the fold, so the sum stays small and
    // its own row violates.
    let fails = build_asserted(
        &[1, 2, 30],
        &[Some(1), Some(-1), Some(-1)],
        &assertion("a <= 2 * SUM(a) FILTER (WHERE b > 0)"),
    );
    let diagnostic = asserted(&fails);
    diagnostic.assert_contains(&["D07", "is false for 1 row", "3"]);
}

#[test]
fn an_aggregate_assertion_passes_vacuously_on_an_empty_table() {
    // Folding nothing yields null, and null passes.
    let yaml = build_asserted(&[], &[], &assertion("SUM(a) > 100"));
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

#[test]
fn columns_applies_the_predicate_to_each_selected_column() {
    let yaml = build_asserted(
        &[1, 2],
        &[Some(1), None],
        &assertion("COLUMNS('a|b') IS NOT NULL"),
    );
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D07", "is false for 1 row", "2"]);
}

#[test]
fn a_column_level_assertion_points_at_its_column() {
    let dir = temp_dir();
    let parquet = dir.join("data.parquet");
    let schema = Arc::new(parse_message_type("message schema { REQUIRED INT64 a; }").unwrap());
    let file = File::create(&parquet).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build()))
            .unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    let mut col = row_group.next_column().unwrap().unwrap();
    col.typed::<Int64Type>()
        .write_batch(&[5, -5], None, None)
        .unwrap();
    col.close().unwrap();
    row_group.close().unwrap();
    writer.close().unwrap();

    let yaml = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: t
                source:
                  parquet: data.parquet
                columns:
                  - name: a
                    type: number(quantity)
                    range: [-100, 100]
                    constraints:
                      - assert: a >= 0
        "},
    );
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D07", "is false for 1 row"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn an_undecodable_column_is_reported_rather_than_passing() {
    // A decimal too wide for exact 64-bit arithmetic is a `number` as far as
    // the metadata level is concerned, but the interpreter can't hold it.
    let dir = temp_dir();
    let parquet = dir.join("data.parquet");
    let schema = Arc::new(
        parse_message_type(
            "message schema { REQUIRED FIXED_LEN_BYTE_ARRAY (16) a (DECIMAL(38,0)); }",
        )
        .unwrap(),
    );
    let file = File::create(&parquet).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build()))
            .unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    let mut col = row_group.next_column().unwrap().unwrap();
    col.typed::<FixedLenByteArrayType>()
        .write_batch(&[FixedLenByteArray::from(vec![0u8; 16])], None, None)
        .unwrap();
    col.close().unwrap();
    row_group.close().unwrap();
    writer.close().unwrap();

    let yaml = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: t
                source:
                  parquet: data.parquet
                constraints:
                  - assert: a > 0
                columns:
                  - name: a
                    type: number(quantity)
                    range: [0, 100]
        "},
    );
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D08", "exact 64-bit form"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn rounding_beyond_a_floats_reach_is_not_an_overflow() {
    // `digits` far enough either way puts the rounding place outside what a
    // float can represent. Both ends have an answer — `x` unchanged, or zero —
    // and neither involves an integer, so neither is D09.
    let yaml = build_asserted(
        &[1234, 5678],
        &[None, None],
        &format!(
            "{}\n{}",
            assertion("ROUND(a, -400) = 0"),
            assertion("ROUND(a, 400) = a")
        ),
    );
    assert_eq!(validate_data(&yaml, None).status(), Status::Ok);
}

/// A two-column table of strings — a subject and the pattern to match it
/// against — with `constraints` spliced in at table level.
fn build_patterned(subject: &[&str], pattern: &[&str], constraints: &str) -> PathBuf {
    let dir = temp_dir();
    let parquet = dir.join("data.parquet");
    let schema = Arc::new(
        parse_message_type(
            "message schema { REQUIRED BYTE_ARRAY s (UTF8); REQUIRED BYTE_ARRAY p (UTF8); }",
        )
        .unwrap(),
    );
    let file = File::create(&parquet).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build()))
            .unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    for values in [subject, pattern] {
        let batch: Vec<ByteArray> = values.iter().map(|v| ByteArray::from(*v)).collect();
        let mut col = row_group.next_column().unwrap().unwrap();
        col.typed::<ByteArrayType>()
            .write_batch(&batch, None, None)
            .unwrap();
        col.close().unwrap();
    }
    row_group.close().unwrap();
    writer.close().unwrap();

    write_dict(
        &dir,
        &formatdoc! {"
            tables:
              - name: t
                source:
                  parquet: data.parquet
                constraints:
            {constraints}
                columns:
                  - name: s
                    type: string
                    examples: [NZ-1234]
                  - name: p
                    type: string
                    examples: ['NZ-.*']
        "},
    )
}

#[test]
fn a_pattern_read_from_the_data_is_matched_per_row() {
    // Each row brings its own pattern, so the compiled regexes must not be
    // confused for one another.
    let yaml = build_patterned(
        &["NZ-1234", "AU-1234", "NZ-9999"],
        &["NZ-.*", "NZ-.*", "NZ-.*"],
        &assertion("s SIMILAR TO p"),
    );
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D07", "is false for 1 row", "2"]);

    let holds = build_patterned(
        &["NZ-1234", "AU-1234"],
        &["NZ-.*", "AU-.*"],
        &assertion("s SIMILAR TO p"),
    );
    assert_eq!(validate_data(&holds, None).status(), Status::Ok);
}

#[test]
fn a_pattern_read_from_the_data_that_is_not_a_regex_is_reported() {
    // A literal pattern is checked at the spec level (S21), but one that comes
    // from the data can only fail here — and must not read as a pass.
    let yaml = build_patterned(
        &["NZ-1234", "AU-1"],
        &["NZ-.*", "*("],
        &assertion("s SIMILAR TO p"),
    );
    let diagnostic = asserted(&yaml);
    diagnostic.assert_contains(&["D08", "not a valid regular expression", "row 2"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}
