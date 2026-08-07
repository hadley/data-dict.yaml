//! Reading columns as the types an expression expects (see `src/typed.rs`).

mod common;

use common::{Fixture, Values};
use data_dict_parquet::{
    Decodable, TypedColumnRequest, TypedValues, ValueType, decodable, read_typed,
};

fn request(path: &str, ty: ValueType) -> TypedColumnRequest {
    TypedColumnRequest {
        path: path.split('.').map(str::to_string).collect(),
        ty,
    }
}

/// Every value of the one requested column, over every batch, with `None` for a
/// null — enough to assert on a small fixture without minding batch boundaries.
fn read_all(path: &std::path::Path, request: TypedColumnRequest) -> Vec<Option<String>> {
    let mut out = Vec::new();
    for batch in read_typed(path, std::slice::from_ref(&request)).unwrap() {
        let batch = batch.unwrap();
        let column = batch.column(0);
        for row in 0..batch.rows() {
            if !column.is_valid(row) {
                out.push(None);
                continue;
            }
            out.push(Some(match column.values() {
                TypedValues::Int(v) => v[row].to_string(),
                TypedValues::Float(v) => v[row].to_string(),
                TypedValues::Bool(v) => v.get(row).to_string(),
                TypedValues::Str(v) => v.get(row).to_string(),
                TypedValues::Date(v) => v[row].to_string(),
                TypedValues::Datetime { micros, .. } => micros[row].to_string(),
            }));
        }
    }
    out
}

#[test]
fn integers_read_as_integers() {
    let path = Fixture::column("OPTIONAL INT64 v", Values::int64([1, 2, 3])).write();
    let values = read_all(&path, request("v", ValueType::Number));
    assert_eq!(
        values,
        [Some("1".into()), Some("2".into()), Some("3".into())]
    );
}

#[test]
fn a_narrower_integer_widens_to_64_bits() {
    let path = Fixture::column("OPTIONAL INT32 v", Values::int32([1, 2])).write();
    assert_eq!(
        read_all(&path, request("v", ValueType::Number)),
        [Some("1".into()), Some("2".into())]
    );
}

#[test]
fn floats_read_as_floats() {
    let path = Fixture::column("OPTIONAL DOUBLE v", Values::double([1.5, -0.25])).write();
    assert_eq!(
        read_all(&path, request("v", ValueType::Number)),
        [Some("1.5".into()), Some("-0.25".into())]
    );
}

#[test]
fn strings_and_booleans_read_back() {
    let path = Fixture::column("OPTIONAL BYTE_ARRAY s (UTF8)", Values::text(["a", "b"])).write();
    assert_eq!(
        read_all(&path, request("s", ValueType::String)),
        [Some("a".into()), Some("b".into())]
    );

    let path = Fixture::column("OPTIONAL BOOLEAN b", Values::bool([true, false])).write();
    assert_eq!(
        read_all(&path, request("b", ValueType::Bool)),
        [Some("true".into()), Some("false".into())]
    );
}

#[test]
fn nulls_are_reported_through_validity() {
    let path = Fixture::column(
        "OPTIONAL INT64 v",
        Values::Int64(vec![Some(1), None, Some(3)]),
    )
    .write();
    assert_eq!(
        read_all(&path, request("v", ValueType::Number)),
        [Some("1".into()), None, Some("3".into())]
    );
}

#[test]
fn a_column_with_no_nulls_says_so() {
    let path = Fixture::column("OPTIONAL INT64 v", Values::int64([1, 2])).write();
    let requests = [request("v", ValueType::Number)];
    let batch = read_typed(&path, &requests)
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert!(batch.column(0).all_valid());
}

#[test]
fn dates_read_as_days_and_datetimes_as_micros() {
    // 1970-01-03 is day 2.
    let path = Fixture::column("OPTIONAL INT32 d (DATE)", Values::int32([2])).write();
    assert_eq!(
        read_all(&path, request("d", ValueType::Date)),
        [Some("2".into())]
    );

    // Milliseconds in, microseconds out.
    let path = Fixture::column(
        "OPTIONAL INT64 t (TIMESTAMP(MILLIS,true))",
        Values::int64([1_500]),
    )
    .write();
    assert_eq!(
        read_all(&path, request("t", ValueType::Datetime)),
        [Some("1500000".into())]
    );
}

#[test]
fn a_zoned_datetime_is_marked_as_an_instant() {
    let zoned = Fixture::column(
        "OPTIONAL INT64 t (TIMESTAMP(MILLIS,true))",
        Values::int64([0]),
    )
    .write();
    let naive = Fixture::column(
        "OPTIONAL INT64 t (TIMESTAMP(MILLIS,false))",
        Values::int64([0]),
    )
    .write();
    for (path, expected) in [(zoned, true), (naive, false)] {
        let requests = [request("t", ValueType::Datetime)];
        let batch = read_typed(&path, &requests)
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let column = batch.column(0);
        let TypedValues::Datetime { utc, .. } = column.values() else {
            panic!("expected a datetime")
        };
        assert_eq!(*utc, expected);
    }
}

#[test]
fn several_columns_come_back_in_request_order() {
    let path = Fixture::new(&["OPTIONAL INT64 a", "OPTIONAL BYTE_ARRAY b (UTF8)"])
        .group(vec![Values::int64([7]), Values::text(["x"])])
        .write();
    let requests = [
        request("b", ValueType::String),
        request("a", ValueType::Number),
    ];
    let batch = read_typed(&path, &requests)
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert!(matches!(batch.column(0).values(), TypedValues::Str(_)));
    assert!(matches!(batch.column(1).values(), TypedValues::Int(_)));
}

#[test]
fn rows_are_numbered_across_batches() {
    // Batches are capped well below this, so the scan yields several and each
    // has to know where it starts for a row number to mean anything.
    const ROWS: i64 = 10_000;
    let path = Fixture::column("OPTIONAL INT64 v", Values::int64(0..ROWS)).write();
    let requests = [request("v", ValueType::Number)];

    let mut expected_first = 0usize;
    let mut batches = 0;
    for batch in read_typed(&path, &requests).unwrap() {
        let batch = batch.unwrap();
        assert_eq!(batch.first_row(), expected_first);
        // The value in each row is its own index, so the numbering is checkable
        // against the data rather than against itself.
        let column = batch.column(0);
        let TypedValues::Int(values) = column.values() else {
            panic!("expected integers")
        };
        for (row, value) in values.iter().enumerate() {
            assert_eq!(*value, (batch.first_row() + row) as i64);
        }
        expected_first += batch.rows();
        batches += 1;
    }
    assert!(batches > 1, "expected more than one batch, got {batches}");
    assert_eq!(expected_first, ROWS as usize);
}

#[test]
fn a_struct_field_is_reachable() {
    // `write_nested` is `group g { INT64 x }`.
    let path = common::write_nested();
    let requests = [request("g.x", ValueType::Number)];
    assert_eq!(decodable(&path, &requests).unwrap(), [Decodable::Yes]);
    assert_eq!(
        read_all(&path, request("g.x", ValueType::Number)),
        [Some("1".into()), Some("2".into())]
    );

    // The struct itself has no value of its own.
    let requests = [request("g", ValueType::Number)];
    assert!(matches!(
        decodable(&path, &requests).unwrap()[..],
        [Decodable::No(_)]
    ));
}

#[test]
fn a_wrong_type_is_reported_before_reading() {
    let path = Fixture::column("OPTIONAL BYTE_ARRAY s (UTF8)", Values::text(["a"])).write();
    let cases = [
        (ValueType::Number, "not a numeric column"),
        (ValueType::Bool, "not a boolean column"),
        (ValueType::Date, "not a date column"),
        (ValueType::Datetime, "not a datetime column"),
    ];
    for (ty, why) in cases {
        let requests = [request("s", ty)];
        assert_eq!(
            decodable(&path, &requests).unwrap(),
            [Decodable::No(why)],
            "{ty:?}"
        );
    }
}

#[test]
fn an_absent_column_is_not_decodable() {
    let path = Fixture::column("OPTIONAL INT64 v", Values::int64([1])).write();
    let requests = [request("nope", ValueType::Number)];
    assert!(matches!(
        decodable(&path, &requests).unwrap()[..],
        [Decodable::No(_)]
    ));
}

#[test]
fn a_value_inside_a_list_is_not_decodable() {
    // A list holds many values per row, and an expression wants one.
    let path = common::write_repeated();
    let requests = [request("xs", ValueType::Number)];
    assert!(matches!(
        decodable(&path, &requests).unwrap()[..],
        [Decodable::No(_)]
    ));
}

#[test]
fn decodable_agrees_with_what_the_scan_does() {
    // The promise made from the footer is the one the scan keeps: anything
    // `decodable` accepts must read, and anything it rejects must not.
    let path = Fixture::new(&[
        "OPTIONAL INT64 n",
        "OPTIONAL BYTE_ARRAY s (UTF8)",
        "OPTIONAL BOOLEAN b",
    ])
    .group(vec![
        Values::int64([1]),
        Values::text(["x"]),
        Values::bool([true]),
    ])
    .write();
    let every = [
        ValueType::Number,
        ValueType::String,
        ValueType::Bool,
        ValueType::Date,
        ValueType::Datetime,
    ];
    for column in ["n", "s", "b"] {
        for ty in every {
            let requests = [request(column, ty)];
            let verdict = decodable(&path, &requests).unwrap();
            let read = read_typed(&path, &requests).is_ok();
            assert_eq!(
                verdict[0] == Decodable::Yes,
                read,
                "{column} as {ty:?}: {verdict:?} but read_typed ok = {read}"
            );
        }
    }
}
