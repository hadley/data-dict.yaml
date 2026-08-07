//! `describe` output: the human-readable text and its `--json` twin.

mod common;

use std::path::Path;

use common::{Fixture, Values, write_nested};
use data_dict_parquet::describe;

/// Snapshots must not carry the unique temp path a fixture is written to.
/// In JSON output serde escapes the backslashes of a Windows path, so that
/// form is replaced too.
fn sanitize(text: &str, path: &Path) -> String {
    let display = path.display().to_string();
    text.replace(&display, "<file>.parquet")
        .replace(&display.replace('\\', "\\\\"), "<file>.parquet")
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

/// Every histogram-generating kind in one file: an integer column narrow
/// enough that whole-number bins mean fewer than 20 of them, a float, a date,
/// UTC and naive timestamps, and a time of day.
#[test]
fn histograms_cover_every_binnable_kind() {
    let path = Fixture::new(&[
        "REQUIRED INT64 count",
        "REQUIRED DOUBLE reading",
        "REQUIRED INT32 event_date (DATE)",
        "REQUIRED INT64 seen_utc (TIMESTAMP(MICROS,true))",
        "REQUIRED INT64 seen_local (TIMESTAMP(MILLIS,false))",
        "REQUIRED INT32 tod (TIME(MILLIS,true))",
    ])
    .group(vec![
        Values::int64([1, 2, 3, 4, 5, 6]),
        Values::double([1.5, 2.5, 2.5, 3.5, 4.5, 5.5]),
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
        // 09:30, 12:00, 17:00.
        Values::int32([
            34_200_000, 34_200_000, 43_200_000, 43_200_000, 61_200_000, 61_200_000,
        ]),
    ])
    .write();
    let description = describe(&path, None).unwrap();
    insta::assert_snapshot!(sanitize(&description.to_string(), &path));
    let json = serde_json::to_string_pretty(&description).unwrap();
    insta::assert_snapshot!(sanitize(&json, &path));
}

#[test]
fn narrows_to_a_single_column() {
    let path = penguins();
    let description = describe(&path, Some("species")).unwrap();
    let text = description.to_string();
    assert!(text.contains("1 column"), "{text}");
    assert!(text.contains("species — string"), "{text}");
    assert!(!text.contains("bill_length_mm"), "{text}");
}

/// A bar whose count rounds below a full cell still shows as a partial
/// block, so a nonzero count is never invisible next to a dominant one.
#[test]
fn tiny_counts_render_a_sliver() {
    let mut values = vec![1i64; 100];
    values.extend([100, 100]);
    let path = Fixture::column("REQUIRED INT64 hits", Values::int64(values)).write();
    let description = describe(&path, None).unwrap();
    let text = description.to_string();
    assert!(text.contains('▎'), "{text}");
    insta::assert_snapshot!(sanitize(&text, &path));
}

/// Past the exact tracker's capacity the distinct count is a sketch estimate:
/// `distinct: ~n`, and a `(~n other values)` tail to match.
#[test]
fn approximate_distinct_counts_are_marked() {
    let values: Vec<Option<String>> = (0..1200).map(|i| Some(format!("visit-{i:04}"))).collect();
    let path = Fixture::column("REQUIRED BYTE_ARRAY visit (UTF8)", Values::Text(values)).write();
    let description = describe(&path, None).unwrap();
    let text = description.to_string();
    assert!(text.contains("~1200"), "{text}");
    insta::assert_snapshot!(sanitize(&text, &path));
}

/// A 200+ character value truncates to the 40-character label budget with an
/// ellipsis, keeping every line on the grid.
#[test]
fn wide_values_truncate_with_an_ellipsis() {
    let wide = "long ".repeat(50); // 250 characters
    let path = Fixture::column(
        "REQUIRED BYTE_ARRAY note (UTF8)",
        Values::text([wide.as_str(), wide.as_str(), "short", "short"]),
    )
    .write();
    let description = describe(&path, None).unwrap();
    let text = sanitize(&description.to_string(), &path);
    assert!(
        text.lines().all(|l| l.chars().count() <= 80),
        "every line should stay on the grid:\n{text}"
    );
    insta::assert_snapshot!(text);
}

/// Embedded newlines and tabs render as escapes instead of breaking the row.
#[test]
fn control_characters_render_as_escapes() {
    let path = Fixture::column(
        "REQUIRED BYTE_ARRAY note (UTF8)",
        Values::text([
            "line one\nline two",
            "line one\nline two",
            "tabbed\there",
            "tabbed\there",
        ]),
    )
    .write();
    let description = describe(&path, None).unwrap();
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
fn many_valued_string_columns_report_the_hidden_tail() {
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
    assert!(text.contains("(10 other values, e.g. "), "{text}");
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

/// Very large integers and floats keep their full decimal form — no
/// scientific notation, no overflow — and the bin range labels still line up
/// once digit counts run into double digits.
#[test]
fn very_large_numbers_render_without_scientific_notation() {
    let path = Fixture::new(&["REQUIRED INT64 population", "REQUIRED DOUBLE budget"])
        .group(vec![
            Values::int64([
                1_000_000_000,
                2_500_000_000,
                7_800_000_000,
                8_100_000_000,
                999_999_999_999,
                1,
            ]),
            Values::double([1.0e12, 2.5e12, 7.8e12, 8.1e12, 9.999e14, 1.0]),
        ])
        .write();
    let description = describe(&path, None).unwrap();
    let text = description.to_string();
    assert!(
        !has_scientific_notation(&text),
        "no scientific notation: {text}"
    );
    insta::assert_snapshot!(sanitize(&text, &path));

    let json = serde_json::to_string_pretty(&description).unwrap();
    assert!(
        !has_scientific_notation(&json),
        "no scientific notation: {json}"
    );
    insta::assert_snapshot!(sanitize(&json, &path));
}

/// A number written in scientific notation has an `e`/`E` with a digit on
/// each side (allowing a sign after it), unlike the word "number" or field
/// names such as "type".
fn has_scientific_notation(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    chars.windows(3).any(|w| {
        (w[1] == 'e' || w[1] == 'E')
            && w[0].is_ascii_digit()
            && (w[2].is_ascii_digit() || w[2] == '+' || w[2] == '-')
    })
}

#[test]
fn unreadable_columns_say_why() {
    let path = write_nested();
    let description = describe(&path, None).unwrap();
    let text = description.to_string();
    assert!(text.contains("not summarised (nested)"), "{text}");
}
