//! `describe` output: the human-readable text and its `--json` twin.

mod common;

use std::path::Path;

use common::{Fixture, Values, write_nested};
use data_dict_parquet::describe;

/// Snapshots must not carry the unique temp path a fixture is written to.
fn sanitize(text: &str, path: &Path) -> String {
    text.replace(&path.display().to_string(), "<file>.parquet")
}

fn penguins() -> std::path::PathBuf {
    Fixture::new(&[
        "REQUIRED BYTE_ARRAY species (UTF8)",
        "OPTIONAL DOUBLE bill_length_mm",
        "REQUIRED INT64 body_mass_g",
        "REQUIRED BOOLEAN alive",
    ])
    .group(vec![
        Values::text([
            "Adelie",
            "Adelie",
            "Adelie",
            "Adelie",
            "Gentoo",
            "Gentoo",
            "Gentoo",
            "Chinstrap",
            "Chinstrap",
        ]),
        Values::Double(vec![
            Some(39.1),
            Some(39.5),
            None,
            Some(36.7),
            Some(46.5),
            Some(50.0),
            None,
            Some(48.7),
            Some(59.6),
        ]),
        Values::int64([3750, 3800, 3250, 3450, 3650, 3625, 4675, 4850, 6300]),
        Values::bool([true, true, true, false, true, false, true, true, true]),
    ])
    .write()
}

#[test]
fn text_covers_strings_floats_ints_and_bools() {
    let path = penguins();
    let description = describe(&path, None).unwrap();
    insta::assert_snapshot!(sanitize(&description.to_string(), &path));
}

#[test]
fn json_matches_the_text() {
    let path = penguins();
    let description = describe(&path, None).unwrap();
    let json = serde_json::to_string_pretty(&description).unwrap();
    insta::assert_snapshot!(sanitize(&json, &path));
}

#[test]
fn temporal_columns_render_iso_labels() {
    let path = Fixture::new(&[
        "REQUIRED INT32 event_date (DATE)",
        "REQUIRED INT64 seen_utc (TIMESTAMP(MICROS,true))",
        "REQUIRED INT64 seen_local (TIMESTAMP(MILLIS,false))",
    ])
    .group(vec![
        // 2020-01-01 .. 2024-01-01.
        Values::int32([18262, 18262, 18628, 19000, 19723, 19723]),
        Values::int64([
            1_577_836_800_000_000,
            1_577_836_800_000_000,
            1_600_000_000_000_000,
            1_600_000_000_000_000,
            1_704_067_199_000_000,
            1_704_067_199_000_000,
        ]),
        Values::int64([
            1_577_865_600_123,
            1_577_865_600_123,
            1_600_000_000_000,
            1_600_000_000_000,
            1_704_096_000_000,
            1_704_096_000_000,
        ]),
    ])
    .write();
    let description = describe(&path, None).unwrap();
    insta::assert_snapshot!(sanitize(&description.to_string(), &path));
    let json = serde_json::to_string_pretty(&description).unwrap();
    insta::assert_snapshot!(sanitize(&sanitize(&json, &path), &path));
}

#[test]
fn narrows_to_a_single_column() {
    let path = penguins();
    let description = describe(&path, Some("species")).unwrap();
    insta::assert_snapshot!(sanitize(&description.to_string(), &path));
}

#[test]
fn unknown_column_lists_the_available_ones() {
    let path = penguins();
    let err = describe(&path, Some("wings")).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("`wings` not found"), "{message}");
    assert!(
        message.contains("species, bill_length_mm, body_mass_g, alive"),
        "{message}"
    );
}

#[test]
fn wide_string_columns_report_the_hidden_tail() {
    // 30 distinct values: the 20 most frequent are shown, 10 are the tail.
    // Nulls too, so the snapshot pins the full ordering of the body's end:
    // value rows, the tail note, then the missing row last.
    let mut values: Vec<Option<String>> = (0..30)
        .flat_map(|i| std::iter::repeat_n(Some(format!("site-{i:02}")), 1 + (30 - i) % 5))
        .collect();
    values.push(None);
    values.push(None);
    let path = Fixture::column("OPTIONAL BYTE_ARRAY site (UTF8)", Values::Text(values)).write();
    let description = describe(&path, None).unwrap();
    let text = description.to_string();
    assert!(text.contains("(10 other values)"), "{text}");
    insta::assert_snapshot!(sanitize(&text, &path));
}

/// A column with no summarisable values has no bars to attach the missing
/// count to, so it stands alone as a plain line — still last.
#[test]
fn all_null_columns_report_missing_alone() {
    let path = Fixture::column(
        "OPTIONAL INT64 reading",
        Values::Int64(vec![None, None, None]),
    )
    .write();
    let description = describe(&path, None).unwrap();
    insta::assert_snapshot!(sanitize(&description.to_string(), &path));
}

#[test]
fn floats_count_nan_apart_from_the_bins() {
    let path = Fixture::column(
        "REQUIRED DOUBLE reading",
        Values::Double(vec![
            Some(1.0),
            Some(2.0),
            Some(3.0),
            Some(f64::NAN),
            Some(f64::NAN),
            Some(f64::INFINITY),
        ]),
    )
    .write();
    let description = describe(&path, None).unwrap();
    let text = description.to_string();
    assert!(text.contains("NaN"), "{text}");
    assert!(text.contains("+inf"), "{text}");
    let json = serde_json::to_string_pretty(&description).unwrap();
    assert!(json.contains("\"nan_count\": 2"), "{json}");
    assert!(json.contains("\"positive_infinity_count\": 1"), "{json}");
    assert!(!json.contains("negative_infinity_count"), "{json}");
}

#[test]
fn unreadable_columns_say_why() {
    let path = write_nested();
    let description = describe(&path, None).unwrap();
    let text = description.to_string();
    assert!(text.contains("not summarised (nested)"), "{text}");
}
