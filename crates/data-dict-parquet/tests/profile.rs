//! Column profiling over generated Parquet files.

mod common;

use std::path::Path;

use common::{Fixture, Values};
use data_dict_parquet::{
    ColumnProfile, Distinct, TimeGrain, Value, ValueCount, ValueKind, profile,
};
use parquet::file::properties::WriterVersion;

fn one(path: &Path) -> ColumnProfile {
    let mut profiles = profile(path, None).unwrap();
    assert_eq!(profiles.len(), 1, "expected a one-column file");
    profiles.remove(0)
}

fn counts(profile: &ColumnProfile) -> Vec<(Value, usize)> {
    profile
        .value_counts
        .iter()
        .map(|count| (count.value.clone(), count.count))
        .collect()
}

#[test]
fn numbers_are_profiled() {
    let path = Fixture::column("REQUIRED INT64 v", Values::int64([1, 2, 2, 3, 3, 3])).write();
    let profile = one(&path);

    assert_eq!(profile.name, "v");
    assert_eq!(profile.kind, ValueKind::Int);
    assert_eq!(profile.row_count, 6);
    assert_eq!(profile.null_count, 0);
    assert_eq!(profile.distinct, Distinct::Exact(3));
    assert_eq!(profile.min, Some(Value::Int(1)));
    assert_eq!(profile.max, Some(Value::Int(3)));
    assert_eq!(
        profile.value_counts[0],
        ValueCount {
            value: Value::Int(3),
            count: 3,
            error: 0
        }
    );
    assert_eq!(
        profile.examples,
        vec![Value::Int(1), Value::Int(2), Value::Int(3)]
    );

    let histogram = profile.histogram.expect("numbers are binnable");
    assert_eq!(histogram.bins.len(), 20);
    assert_eq!(histogram.nan_count, 0);
    assert_eq!(histogram.bins.iter().map(|bin| bin.count).sum::<usize>(), 6);
    assert_eq!(histogram.bins[0].count, 1);
    assert_eq!(histogram.bins[19].count, 3);
}

#[test]
fn strings_are_profiled_but_not_binned() {
    let path = Fixture::column(
        "REQUIRED BYTE_ARRAY v (UTF8)",
        Values::text(["otter", "seal", "otter"]),
    )
    .write();
    let profile = one(&path);

    assert_eq!(profile.kind, ValueKind::Text);
    assert_eq!(profile.distinct, Distinct::Exact(2));
    assert_eq!(profile.min, Some(Value::Text("otter".into())));
    assert_eq!(profile.max, Some(Value::Text("seal".into())));
    assert_eq!(counts(&profile)[0], (Value::Text("otter".into()), 2));
    assert!(profile.histogram.is_none(), "text has no numeric scale");
}

#[test]
fn booleans_have_counts_but_no_order() {
    let path = Fixture::column(
        "REQUIRED BOOLEAN v",
        Values::bool([true, true, true, false]),
    )
    .write();
    let profile = one(&path);

    assert_eq!(profile.kind, ValueKind::Bool);
    assert_eq!(profile.distinct, Distinct::Exact(2));
    assert_eq!(profile.min, None);
    assert_eq!(profile.max, None);
    assert_eq!(profile.histogram, None);
    assert_eq!(counts(&profile)[0], (Value::Bool(true), 3));
}

#[test]
fn dates_and_timestamps_keep_their_units() {
    let path = Fixture::column("REQUIRED INT32 d (DATE)", Values::int32([19000, 19010])).write();
    let profile = one(&path);
    assert_eq!(profile.kind, ValueKind::Date);
    assert_eq!(profile.min, Some(Value::Int(19000)));
    assert!(profile.histogram.is_some(), "dates are binnable");

    for (field, utc_adjusted) in [
        ("REQUIRED INT64 t (TIMESTAMP(MICROS,true))", true),
        ("REQUIRED INT64 t (TIMESTAMP(MICROS,false))", false),
    ] {
        let path = Fixture::column(field, Values::int64([1_700_000_000_000_000])).write();
        assert_eq!(
            one(&path).kind,
            ValueKind::Timestamp {
                grain: TimeGrain::Micros,
                utc_adjusted,
            }
        );
    }
}

#[test]
fn nulls_are_counted_and_never_binned() {
    let values = Values::Int64(vec![Some(1), None, Some(3), None, None]);
    let path = Fixture::column("OPTIONAL INT64 v", values).write();
    let profile = one(&path);

    assert_eq!(profile.row_count, 5);
    assert_eq!(profile.null_count, 3);
    assert_eq!(profile.distinct, Distinct::Exact(2));
    let histogram = profile.histogram.unwrap();
    assert_eq!(histogram.bins.iter().map(|bin| bin.count).sum::<usize>(), 2);
}

/// The value scan is always correct, so the two paths agreeing is what pins the
/// dictionary shortcut down. Kept under the 1,000-value tracking cap, where
/// counts are exact and so independent of the order values arrive in.
#[test]
fn the_dictionary_and_value_paths_agree() {
    for version in [WriterVersion::PARQUET_1_0, WriterVersion::PARQUET_2_0] {
        for (field, values) in agreement_cases() {
            let dictionary = one(&Fixture::column(field, values.clone())
                .dictionary(true)
                .version(version)
                .write());
            let plain = one(&Fixture::column(field, values)
                .dictionary(false)
                .version(version)
                .write());
            assert_eq!(dictionary, plain, "{version:?} paths disagree for {field}");
        }
    }
}

fn agreement_cases() -> Vec<(&'static str, Values)> {
    vec![
        ("REQUIRED INT64 v", Values::int64([1, 2, 2, 9, -4, 9, 9])),
        (
            "REQUIRED INT32 v (DATE)",
            Values::int32([19000, 19001, 19000]),
        ),
        ("REQUIRED DOUBLE v", Values::double([1.5, 2.5, 1.5, -0.5])),
        (
            "REQUIRED BYTE_ARRAY v (UTF8)",
            Values::text(["a", "b", "a", "c", "a"]),
        ),
        ("REQUIRED BOOLEAN v", Values::bool([true, false, true])),
        (
            "OPTIONAL INT64 v",
            Values::Int64(vec![Some(1), None, Some(1), Some(7), None]),
        ),
        (
            "OPTIONAL BYTE_ARRAY v (UTF8)",
            Values::Text(vec![Some("x".into()), None, Some("y".into())]),
        ),
    ]
}

#[test]
fn row_groups_are_combined() {
    let path = Fixture::new(&["OPTIONAL INT64 v"])
        .group(vec![Values::Int64(vec![Some(1), Some(2), None])])
        .group(vec![Values::Int64(vec![Some(2), Some(30)])])
        .write();
    let profile = one(&path);

    assert_eq!(profile.row_count, 5);
    assert_eq!(profile.null_count, 1);
    assert_eq!(profile.distinct, Distinct::Exact(3));
    assert_eq!(profile.min, Some(Value::Int(1)));
    assert_eq!(profile.max, Some(Value::Int(30)));
    assert_eq!(counts(&profile)[0], (Value::Int(2), 2));
}

/// A chunk that starts dictionary-encoded and gives up partway through must
/// fall back for the whole chunk, counting every value exactly once.
#[test]
fn a_chunk_that_abandons_its_dictionary_is_still_exact() {
    let values: Vec<i64> = (0..500).map(|row| row % 200).collect();
    let path = Fixture::column("REQUIRED INT64 v", Values::int64(values))
        .small_pages()
        .write();
    let profile = one(&path);

    assert_eq!(profile.row_count, 500);
    assert_eq!(profile.distinct, Distinct::Exact(200));
    assert_eq!(profile.min, Some(Value::Int(0)));
    assert_eq!(profile.max, Some(Value::Int(199)));
    assert_eq!(counts(&profile)[0].1, 3);
}

#[test]
fn a_missing_footer_range_is_recovered_by_a_second_pass() {
    let values: Vec<i64> = (1..=100).collect();
    let with = one(&Fixture::column("REQUIRED INT64 v", Values::int64(values.clone())).write());
    let without = one(&Fixture::column("REQUIRED INT64 v", Values::int64(values))
        .statistics(false)
        .write());

    assert_eq!(with.histogram, without.histogram);
    let histogram = with.histogram.unwrap();
    assert_eq!(histogram.bins.len(), 20);
    assert_eq!(
        histogram.bins.iter().map(|bin| bin.count).sum::<usize>(),
        100
    );
    assert_eq!(histogram.bins[0].lower, 1.0);
    assert_eq!(histogram.bins[19].upper, 100.0);
}

/// Past the tracking cap the counts stop being exact, but a value that only
/// starts arriving after the cap is reached must still surface.
#[test]
fn a_late_heavy_hitter_survives_saturation() {
    let mut values: Vec<i64> = (0..1_200).collect();
    values.extend(std::iter::repeat_n(900_001, 3_000));
    values.extend(std::iter::repeat_n(900_002, 2_000));
    values.extend(1_200..5_000);

    let path = Fixture::column("REQUIRED INT64 v", Values::int64(values)).write();
    let profile = one(&path);

    let Distinct::Approx(estimate) = profile.distinct else {
        panic!("5,002 distinct values must exceed the tracking cap");
    };
    let error = (estimate as f64 - 5_002.0).abs() / 5_002.0;
    assert!(error < 0.05, "estimated {estimate} distinct");

    for (value, truth) in [(900_001, 3_000), (900_002, 2_000)] {
        let found = profile
            .value_counts
            .iter()
            .find(|count| count.value == Value::Int(value))
            .unwrap_or_else(|| panic!("{value} missing from the top values"));
        assert!(
            found.count - found.error <= truth && truth <= found.count,
            "{value}: {} ± {} does not bracket {truth}",
            found.count,
            found.error
        );
    }
}

#[test]
fn examples_spread_along_the_distinct_values() {
    let path = Fixture::column("REQUIRED INT64 v", Values::int64(1..=101)).write();
    assert_eq!(
        one(&path).examples,
        vec![
            Value::Int(1),
            Value::Int(26),
            Value::Int(51),
            Value::Int(76),
            Value::Int(101)
        ]
    );

    // A repeated value is no likelier to be sampled than a rare one.
    let mut values: Vec<i64> = vec![7; 10_000];
    values.extend([1, 2, 3]);
    let path = Fixture::column("REQUIRED INT64 v", Values::int64(values)).write();
    assert_eq!(
        one(&path).examples,
        vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(7)]
    );
}

#[test]
fn nans_are_counted_apart_from_nulls_and_values() {
    let values = Values::Double(vec![
        Some(1.0),
        Some(f64::NAN),
        Some(3.0),
        None,
        Some(f64::NAN),
    ]);
    let path = Fixture::column("OPTIONAL DOUBLE v", values).write();
    let profile = one(&path);

    assert_eq!(profile.null_count, 1);
    assert_eq!(profile.distinct, Distinct::Exact(2), "NaN is not a value");
    assert_eq!(profile.min.and_then(|v| v.as_f64()), Some(1.0));
    assert_eq!(profile.max.and_then(|v| v.as_f64()), Some(3.0));
    let histogram = profile.histogram.unwrap();
    assert_eq!(histogram.nan_count, 2);
    assert_eq!(histogram.bins.iter().map(|bin| bin.count).sum::<usize>(), 2);
}

#[test]
fn a_column_of_nothing_but_nans_has_no_range() {
    let values = Values::Double(vec![Some(f64::NAN), Some(f64::NAN)]);
    let path = Fixture::column("REQUIRED DOUBLE v", values).write();
    let profile = one(&path);

    assert_eq!(profile.min, None);
    assert_eq!(profile.max, None);
    assert_eq!(profile.distinct, Distinct::Exact(0));
    let histogram = profile
        .histogram
        .expect("the NaNs are still worth reporting");
    assert!(histogram.bins.is_empty());
    assert_eq!(histogram.nan_count, 2);
}

#[test]
fn signed_zeros_are_one_value() {
    let path = Fixture::column("REQUIRED DOUBLE v", Values::double([0.0, -0.0, 0.0])).write();
    let profile = one(&path);

    assert_eq!(profile.distinct, Distinct::Exact(1));
    assert_eq!(counts(&profile)[0].1, 3);
    let histogram = profile.histogram.unwrap();
    assert_eq!(histogram.bins.len(), 1, "no range to divide");
    assert_eq!(histogram.bins[0].count, 3);
}

#[test]
fn a_single_repeated_value_gets_one_bin() {
    let path = Fixture::column("REQUIRED INT64 v", Values::int64([5, 5, 5])).write();
    let histogram = one(&path).histogram.unwrap();

    assert_eq!(histogram.bins.len(), 1);
    assert!(histogram.bins[0].lower_inclusive);
    assert_eq!(
        (histogram.bins[0].lower, histogram.bins[0].upper),
        (5.0, 5.0)
    );
    assert_eq!(histogram.bins[0].count, 3);
}

#[test]
fn an_all_null_column_reports_only_nulls() {
    let values = Values::Int64(vec![None, None, None]);
    let path = Fixture::column("OPTIONAL INT64 v", values).write();
    let profile = one(&path);

    assert_eq!(profile.row_count, 3);
    assert_eq!(profile.null_count, 3);
    assert_eq!(profile.distinct, Distinct::Exact(0));
    assert_eq!(profile.min, None);
    assert_eq!(profile.histogram, None);
    assert!(profile.value_counts.is_empty());
    assert!(profile.examples.is_empty());
}

#[test]
fn an_empty_file_profiles_to_nothing() {
    let path = Fixture::new(&["OPTIONAL INT64 v"]).write();
    let profile = one(&path);

    assert_eq!(profile.row_count, 0);
    assert_eq!(profile.null_count, 0);
    assert_eq!(profile.distinct, Distinct::Exact(0));
    assert_eq!(profile.min, None);
    assert_eq!(profile.histogram, None);
}

#[test]
fn only_the_requested_columns_are_profiled() {
    let path = Fixture::new(&["REQUIRED INT64 a", "REQUIRED BYTE_ARRAY b (UTF8)"])
        .group(vec![Values::int64([1, 2]), Values::text(["x", "y"])])
        .write();

    let all = profile(&path, None).unwrap();
    assert_eq!(
        all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );

    let selected = profile(&path, Some(&["b"])).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].name, "b");

    let missing = profile(&path, Some(&["nope"]));
    assert!(missing.is_err(), "an unknown column must be an error");
}

#[test]
fn unprofilable_types_report_row_counts_only() {
    let path = Fixture::column(
        "OPTIONAL INT64 amount (DECIMAL(9,2))",
        Values::Int64(vec![Some(150), None, Some(275)]),
    )
    .write();
    let profile = one(&path);

    assert_eq!(profile.kind, ValueKind::Unsupported("decimal"));
    assert_eq!(profile.row_count, 3);
    assert_eq!(profile.null_count, 1, "taken from the footer");
    assert_eq!(profile.min, None);
    assert!(profile.value_counts.is_empty());
}

#[test]
fn a_column_inside_a_group_is_not_profiled() {
    let path = common::write_nested();
    let profile = one(&path);

    assert_eq!(profile.name, "g");
    assert_eq!(profile.kind, ValueKind::Unsupported("nested"));
    assert_eq!(profile.row_count, 2);
}

/// Only reading the values can prove a byte array is text, so the column is
/// abandoned partway through — without taking the rest of the file with it.
#[test]
fn a_byte_array_that_is_not_text_is_abandoned() {
    let path = Fixture::new(&["OPTIONAL BYTE_ARRAY raw", "REQUIRED INT64 n"])
        .group(vec![
            Values::Bytes(vec![Some(b"fine".to_vec()), Some(vec![0xff, 0xfe])]),
            Values::int64([1, 2]),
        ])
        // The scan gives up in the first row group, so nulls counted as it went
        // would miss this one entirely.
        .group(vec![Values::Bytes(vec![None, None]), Values::int64([3, 4])])
        .write();
    let profiles = profile(&path, None).unwrap();

    assert_eq!(profiles[0].kind, ValueKind::Unsupported("non-UTF-8"));
    assert_eq!(profiles[0].row_count, 4);
    assert_eq!(profiles[0].null_count, 2, "taken from the footer");
    assert!(profiles[0].value_counts.is_empty());
    assert_eq!(profiles[1].kind, ValueKind::Int);
    assert_eq!(profiles[1].distinct, Distinct::Exact(4));
}
