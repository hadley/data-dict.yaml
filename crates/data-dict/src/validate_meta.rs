//! Metadata-level validation, the `M##` checks (see `site/validation.md`).
//!
//! [`validate_meta`] is the entry point; [`meta_issues`] is the reusable core
//! that the data level ([`crate::validate_data`]) runs before its own value checks.
//! The source checks (M04, M05) live in [`crate::compare_dataset`], which locates
//! and reads each table's data before these column checks run.

use std::path::Path;

use data_dict_parquet::ColumnMeta;

use crate::model::{Column, Constraint, Table};
use crate::problem::{Problem, ProblemKind, ProblemSet, Severity};

/// The result of attempting a data-level check from metadata alone.
pub(crate) enum CheckResult {
    Pass,
    Inconclusive,
    Fail(Box<Problem>),
}

/// Validate every table's column names and types against a data dictionary.
///
/// Validates the spec first, then — when it is free of errors — compares each
/// table's `source` data against its dictionary entry, reporting type
/// mismatches, columns described but absent from the data, and columns in the
/// data the dictionary does not describe. Values are never read; see
/// [`crate::validate_data::validate_data`] for the level that does.
pub fn validate_meta(dict_path: &Path, table: Option<&str>) -> ProblemSet {
    crate::compare_dataset(
        dict_path,
        table,
        |table, _parquet, actual, problems| {
            meta_issues(table, actual, problems);
        },
        |_dict, _readable, _problems| {},
    )
}

/// Compare the dictionary's `table` against the actual column types read from
/// the data, pushing the metadata-level problems into `out`. Reused by the data
/// level, which appends its value-level problems to the same set.
pub(crate) fn meta_issues(table: &Table, actual: &[(String, String)], out: &mut ProblemSet) {
    validate_m01_column_types(table, actual, out);
    validate_m02_missing_columns(table, actual, out);
    validate_m03_extra_columns(table, actual, out);
}

/// Attempt D01 from Parquet footer metadata. Although this reads only metadata,
/// the rule remains a D## check because it validates the column's values.
pub(crate) fn validate_d01_required_not_null(
    table: &Table,
    col: &Column,
    meta: &ColumnMeta,
) -> CheckResult {
    if !col.is_required_implied() {
        return CheckResult::Pass;
    }
    match meta.null_count {
        Some(0) => CheckResult::Pass,
        Some(count) => CheckResult::Fail(Box::new(nulls_in_required_meta(table, col, count))),
        None => CheckResult::Inconclusive,
    }
}

/// Attempt D04 from footer metadata. Set membership can't be settled from the
/// footer — min/max bound the extremes but say nothing about the values between
/// them — so an `enum` column carrying a `values` set is always inconclusive and
/// deferred to the scan; any other column passes here.
pub(crate) fn validate_d04_enum_membership(col: &Column) -> CheckResult {
    if col.is_enum() && col.values.is_some() {
        CheckResult::Inconclusive
    } else {
        CheckResult::Pass
    }
}

/// Attempt the individual-column form of D02 from footer statistics.
pub(crate) fn validate_d02_unique_column(
    table: &Table,
    col: &Column,
    meta: &ColumnMeta,
) -> CheckResult {
    if !col.has(Constraint::Unique) {
        return CheckResult::Pass;
    }
    let (Some(distinct), Some(nulls)) = (meta.distinct_count, meta.null_count) else {
        return CheckResult::Inconclusive;
    };
    // Parquet writers differ in how they populate distinct counts around nulls;
    // scan nullable data rather than drawing an unsafe footer-only conclusion.
    if nulls > 0 {
        return CheckResult::Inconclusive;
    }
    if distinct == meta.row_count {
        CheckResult::Pass
    } else if distinct < meta.row_count {
        CheckResult::Fail(Box::new(duplicates_meta(
            table,
            col,
            meta.row_count - distinct,
        )))
    } else {
        CheckResult::Inconclusive
    }
}

fn nulls_in_required_meta(table: &Table, col: &Column, count: usize) -> Problem {
    let plural = if count == 1 { "" } else { "s" };
    let constraint_span = col
        .constraints
        .iter()
        .find(|constraint| {
            matches!(
                constraint.value,
                Constraint::Required | Constraint::PrimaryKey
            )
        })
        .map_or_else(
            || col.name.span.clone(),
            |constraint| constraint.span.clone(),
        );
    Problem {
        code: Some("D01"),
        severity: Severity::Error,
        message: format!("has {count} null value{plural}"),
        column: None,
        expected: Some("A required column must not contain nulls.".into()),
        hint: None,
        suggestion: None,
        context: vec![
            table.name.span.clone(),
            col.name.span.clone(),
            constraint_span,
        ],
        kind: ProblemKind::NullsInRequired {
            count,
            rows: Vec::new(),
        },
    }
}

fn duplicates_meta(table: &Table, col: &Column, count: usize) -> Problem {
    let plural = if count == 1 { "" } else { "s" };
    let constraint_span = col
        .constraints
        .iter()
        .find(|constraint| constraint.value == Constraint::Unique)
        .map_or_else(
            || col.name.span.clone(),
            |constraint| constraint.span.clone(),
        );
    Problem {
        code: Some("D02"),
        severity: Severity::Error,
        message: format!("has {count} repeated occurrence{plural}"),
        column: None,
        expected: Some("A unique column must not contain duplicate values.".into()),
        hint: None,
        suggestion: None,
        context: vec![
            table.name.span.clone(),
            col.name.span.clone(),
            constraint_span,
        ],
        kind: ProblemKind::DuplicateValues {
            columns: vec![col.name.value.clone()],
            count,
            rows: Vec::new(),
        },
    }
}

fn validate_m01_column_types(table: &Table, actual: &[(String, String)], out: &mut ProblemSet) {
    for col in &table.columns {
        // Only described columns present in the data are type-checked; an absent
        // column is M02's concern, and a column with no `type` makes no claims.
        let Some((_, actual_type)) = actual.iter().find(|(n, _)| n == &col.name.value) else {
            continue;
        };
        if let Some(declared) = &col.col_type
            && !types_compatible(&declared.value, actual_type)
        {
            out.push_located(
                ProblemKind::TypeMismatch {
                    declared: declared.value.clone(),
                    actual: actual_type.to_string(),
                },
                Severity::Error,
                "A column's data must match its declared type.",
                format!("the data is `{actual_type}`"),
                [
                    table.name.span.clone(),
                    col.name.span.clone(),
                    declared.span.clone(),
                ],
            );
        }
    }
}

fn validate_m02_missing_columns(table: &Table, actual: &[(String, String)], out: &mut ProblemSet) {
    for col in &table.columns {
        if !actual.iter().any(|(n, _)| n == &col.name.value) {
            out.push_located(
                ProblemKind::MissingInData,
                Severity::Error,
                "Every column in the dictionary must be present in the data.",
                "is missing from the data",
                [table.name.span.clone(), col.name.span.clone()],
            );
        }
    }
}

fn validate_m03_extra_columns(table: &Table, actual: &[(String, String)], out: &mut ProblemSet) {
    for (name, actual_type) in actual {
        if table.column(name).is_none() {
            // The column exists only in the data, so there is no dictionary node
            // to point at; it is named in the message instead.
            out.push(Problem::undocumented_column(name, actual_type.clone()));
        }
    }
}

/// Collapse a declared dictionary type to its base form for comparison by
/// dropping any trailing `(...)` qualifier.
fn normalize_dict_type(dict_type: &str) -> &str {
    match dict_type.find('(') {
        Some(i) => &dict_type[..i],
        None => dict_type,
    }
}

/// Whether a declared dictionary type is compatible with a type read from the
/// data (one of `boolean`, `string`, `enum`, `date`, `datetime`, `number`).
///
/// Dictionary types are coarser/richer than physical types, so the match is by
/// category rather than exact string. An `enum` must be backed by string-like
/// data — a string column or a true parquet enum — never a number.
fn types_compatible(dict_type: &str, actual: &str) -> bool {
    match normalize_dict_type(dict_type) {
        "number" => actual == "number",
        "string" => actual == "string",
        "boolean" => actual == "boolean",
        "date" => actual == "date",
        "datetime" => actual == "datetime",
        "enum" => matches!(actual, "string" | "enum"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_measures_normalize() {
        assert_eq!(normalize_dict_type("number(quantity)"), "number");
        assert_eq!(normalize_dict_type("number(id)"), "number");
        assert_eq!(normalize_dict_type("number"), "number");
        assert_eq!(normalize_dict_type("string"), "string");
    }

    #[test]
    fn compatibility() {
        assert!(types_compatible("number(quantity)", "number"));
        assert!(types_compatible("string", "string"));
        assert!(types_compatible("enum", "string"));
        assert!(!types_compatible("enum", "number"));
        assert!(types_compatible("enum", "enum"));
        assert!(!types_compatible("number", "string"));
        assert!(!types_compatible("date", "datetime"));
        assert!(!types_compatible("boolean", "number"));
    }
}
