//! Metadata-level validation, the `M##` checks (see `site/validation.md`).
//!
//! [`validate_meta`] is the entry point; [`meta_issues`] is the reusable core
//! that the data level ([`crate::validate_data`]) runs before its own value checks.
//! The source checks (M04, M05) live in [`crate::compare_dataset`], which locates
//! and reads each table's data before these column checks run.

use std::path::Path;

use data_dict_parquet::{ColumnMeta, DataColumn};

use crate::Level;
use crate::model::{Column, Constraint, Table};
use crate::problem::{Problem, ProblemKind, ProblemSet, Severity};
use crate::report::{Failed, StepKey, StepTarget};

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
        Level::Meta,
        |table, _parquet, actual, problems| {
            meta_issues(table, actual, problems);
        },
        |_dict, _readable, _problems| {},
    )
}

/// Compare the dictionary's `table` against the actual column types read from
/// the data, pushing the metadata-level problems into `out`. Reused by the data
/// level, which appends its value-level problems to the same set. The M01–M03
/// checks descend into `struct` fields (including through `list(struct)`),
/// each declared field checked against the corresponding child of the data.
pub(crate) fn meta_issues(table: &Table, actual: &[DataColumn], out: &mut ProblemSet) {
    check_columns(table, &table.columns, actual, &[], out);
}

/// Run M01–M03 for one level of the column tree: `declared` against `actual`,
/// where `path` holds the enclosing struct columns' names (empty at the top
/// level, so problems name fields by their dotted path).
fn check_columns(
    table: &Table,
    declared: &[Column],
    actual: &[DataColumn],
    path: &[&str],
    out: &mut ProblemSet,
) {
    for col in declared {
        let segments: Vec<String> = path
            .iter()
            .map(|s| (*s).to_string())
            .chain([col.name.value.clone()])
            .collect();
        let dotted = segments.join(".");
        let step = StepKey::new(&table.name.value, StepTarget::Column(segments));
        // An absent column is M02's concern; a column with no `type` makes no
        // claims, but its declared fields (if any) are still checked.
        let Some(data) = actual.iter().find(|c| c.name == col.name.value) else {
            validate_m02_missing(table, col, path, out);
            out.at_last(&table.name.value, [dotted]);
            out.fail_last(&step, Failed::AllRows);
            continue;
        };
        // A type mismatch is the root cause; don't cascade into per-field
        // reports against data of the wrong shape. The fields' own steps are
        // left unevaluated, since nothing compared them.
        if !validate_m01_column_type(table, col, data, out) {
            out.at_last(&table.name.value, [dotted]);
            out.fail_last(&step, Failed::AllRows);
            continue;
        }
        out.step_pass(&step);
        if let Some(fields) = &col.fields {
            let path: Vec<&str> = path
                .iter()
                .copied()
                .chain([col.name.value.as_str()])
                .collect();
            check_columns(table, fields, &data.children, &path, out);
        }
    }
    validate_m03_extra_columns(table, declared, actual, path, out);
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
        step: None,
        severity: Severity::Error,
        message: format!("has {count} null value{plural}"),
        table: Some(table.name.value.clone()),
        columns: vec![col.name.value.clone()],
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
        step: None,
        severity: Severity::Error,
        message: format!("has {count} repeated occurrence{plural}"),
        table: Some(table.name.value.clone()),
        columns: vec![col.name.value.clone()],
        expected: Some("A unique column must not contain duplicate values.".into()),
        hint: None,
        suggestion: None,
        context: vec![
            table.name.span.clone(),
            col.name.span.clone(),
            constraint_span,
        ],
        kind: ProblemKind::DuplicateValues {
            count,
            rows: Vec::new(),
            // The footer proves how many rows repeat but not which, so there is
            // no row to read a value from.
            values: Vec::new(),
            redacted: false,
        },
    }
}

/// M01 for one column or field. Returns whether the declared type made no
/// claim or was compatible, so the caller knows the data is the declared shape
/// and any declared fields can be checked against its children.
fn validate_m01_column_type(
    table: &Table,
    col: &Column,
    data: &DataColumn,
    out: &mut ProblemSet,
) -> bool {
    let Some(declared) = &col.col_type else {
        return true;
    };
    if types_compatible(&declared.value, &data.dict_type) {
        return true;
    }
    out.push_located(
        ProblemKind::TypeMismatch {
            declared: declared.value.clone(),
            actual: data.dict_type.clone(),
        },
        Severity::Error,
        "A column's data must match its declared type.",
        format!("the data is `{}`", data.dict_type),
        [
            table.name.span.clone(),
            col.name.span.clone(),
            declared.span.clone(),
        ],
    );
    false
}

fn validate_m02_missing(table: &Table, col: &Column, path: &[&str], out: &mut ProblemSet) {
    let (expected, message) = if path.is_empty() {
        (
            "Every column in the dictionary must be present in the data.",
            "is missing from the data".to_string(),
        )
    } else {
        (
            "Every field in the dictionary must be present in the data's struct.",
            format!("is missing from the data's `{}` struct", path.join(".")),
        )
    };
    out.push_located(
        ProblemKind::MissingInData,
        Severity::Error,
        expected,
        message,
        [table.name.span.clone(), col.name.span.clone()],
    );
}

fn validate_m03_extra_columns(
    table: &Table,
    declared: &[Column],
    actual: &[DataColumn],
    path: &[&str],
    out: &mut ProblemSet,
) {
    for data in actual {
        if !declared.iter().any(|c| c.name.value == data.name) {
            // The column exists only in the data, so there is no dictionary node
            // to point at; it is named in the message instead, by its dotted
            // path when it is a field.
            let name = path
                .iter()
                .copied()
                .chain([data.name.as_str()])
                .collect::<Vec<_>>()
                .join(".");
            out.push(Problem::undocumented_column(
                &table.name.value,
                &name,
                data.dict_type.clone(),
            ));
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

/// The element type of a `list(...)` type string, from either side of the
/// comparison.
fn list_element(type_name: &str) -> Option<&str> {
    type_name.strip_prefix("list(")?.strip_suffix(")")
}

/// Whether a declared dictionary type is compatible with a type read from the
/// data (one of `boolean`, `string`, `enum`, `date`, `datetime`, `number`,
/// `struct`, `list(...)`, or a shape with no data-dict type such as `map`).
///
/// Dictionary types are coarser/richer than physical types, so the match is by
/// category rather than exact string. An `enum` must be backed by string-like
/// data — a string column or a true parquet enum — never a number. A list
/// matches list-shaped data whose element type is compatible.
fn types_compatible(dict_type: &str, actual: &str) -> bool {
    if let Some(elem) = list_element(dict_type) {
        return list_element(actual).is_some_and(|actual_elem| types_compatible(elem, actual_elem));
    }
    match normalize_dict_type(dict_type) {
        "number" => actual == "number",
        "string" => actual == "string",
        "boolean" => actual == "boolean",
        "date" => actual == "date",
        "datetime" => actual == "datetime",
        "enum" => matches!(actual, "string" | "enum"),
        "struct" => actual == "struct",
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

    #[test]
    fn nested_compatibility() {
        assert!(types_compatible("struct", "struct"));
        assert!(!types_compatible("struct", "number"));
        assert!(types_compatible("list(string)", "list(string)"));
        assert!(types_compatible("list(enum)", "list(string)"));
        assert!(types_compatible("list(number(quantity))", "list(number)"));
        assert!(types_compatible("list(struct)", "list(struct)"));
        assert!(!types_compatible("list(string)", "string"));
        assert!(!types_compatible("string", "list(string)"));
        assert!(!types_compatible("list(number)", "list(string)"));
        // A map has no data-dict type, so nothing declared matches it.
        assert!(!types_compatible("struct", "map"));
        assert!(!types_compatible("list(struct)", "map"));
    }

    #[test]
    fn nested_list_compatibility() {
        assert!(types_compatible("list(list(string))", "list(list(string))"));
        assert!(types_compatible("list(list(enum))", "list(list(string))"));
        assert!(types_compatible(
            "list(list(number(quantity)))",
            "list(list(number))"
        ));
        assert!(types_compatible("list(list(struct))", "list(list(struct))"));
        // Depths must agree.
        assert!(!types_compatible("list(string)", "list(list(string))"));
        assert!(!types_compatible("list(list(string))", "list(string)"));
    }
}
