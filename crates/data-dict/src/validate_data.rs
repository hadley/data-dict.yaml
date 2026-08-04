//! Data-level validation, the `D##` checks (see `site/validation.md`).
//!
//! [`validate_data`] is the entry point; `value_issues` is the value-checking
//! core it runs after the metadata checks ([`crate::validate_meta`]).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use data_dict_parquet::{
    ColumnMeta, ColumnNeeds, ColumnStats, ForeignKeyCheck, ForeignKeyResult, ForeignKeyStats,
    UniquenessCheck, UniquenessStats,
};

use crate::ReadTables;
use crate::model::{Column, Constraint, DataDict, Table};
use crate::problem::{Problem, ProblemKind, ProblemSet, Severity};
use crate::validate_meta::CheckResult;

/// How many example values (e.g. offending rows) to record per validation
/// issue. Issues count every offender but only list this many.
const SAMPLE_LIMIT: usize = 5;

/// Validate a parquet file's values against a data dictionary.
///
/// Validates the spec first, then — when it is free of errors — runs every
/// metadata-level check ([`crate::validate_meta`]) plus the value-level checks
/// below: reading the columns and pages the checks imply and reporting, for
/// example, nulls in a required column.
pub fn validate_data(dict_path: &Path, table: Option<&str>) -> ProblemSet {
    crate::compare_dataset(
        dict_path,
        table,
        |table, parquet_path, actual, problems| {
            crate::validate_meta::meta_issues(table, actual, problems);
            if let Err(e) = value_issues(table, parquet_path, actual, problems) {
                problems.push(Problem::preflight(ProblemKind::Parquet, e.to_string()));
            }
        },
        foreign_key_issues,
    )
}

/// Run the value-level checks for the dictionary's `table` against the data,
/// pushing any problems found into `out`. `actual` is the column schema already
/// read for the metadata checks, used here only to tell which columns are
/// present.
fn value_issues(
    table: &Table,
    parquet_path: &Path,
    actual: &[(String, String)],
    out: &mut ProblemSet,
) -> Result<(), data_dict_parquet::ParquetError> {
    let present = |name: &str| actual.iter().any(|(an, _)| an == name);
    let metadata = data_dict_parquet::column_meta(parquet_path)?;

    // Phase 1 — check the footer. A data-level rule remains D## even when
    // Parquet metadata is sufficient to prove its result. Only inconclusive
    // checks are allowed to request a value scan.
    let mut needs: HashMap<String, ColumnNeeds> = HashMap::new();
    let mut pending: HashMap<String, Vec<&dyn ColumnCheck>> = HashMap::new();
    for col in &table.columns {
        let Some(actual_type) = actual
            .iter()
            .find(|(name, _)| name == &col.name.value)
            .map(|(_, actual_type)| actual_type.as_str())
        else {
            continue;
        };
        let Some(meta) = metadata.get(&col.name.value) else {
            continue;
        };
        let mut merged = ColumnNeeds::default();
        for check in VALUE_CHECKS {
            match check.check_meta(table, col, meta) {
                CheckResult::Pass => {}
                CheckResult::Inconclusive => {
                    merged = merged.merge(check.needs(col, actual_type));
                    pending
                        .entry(col.name.value.clone())
                        .or_default()
                        .push(*check);
                }
                CheckResult::Fail(problem) => out.push(*problem),
            }
        }
        if merged.any() {
            needs.insert(col.name.value.clone(), merged);
        }
    }

    // Phase 2 — scan. Gather exactly those statistics, in one pass, reading only
    // the columns and pages the plan implies.
    let stats = data_dict_parquet::column_stats(parquet_path, &needs, SAMPLE_LIMIT)?;

    // Phase 3 — check. Per column with gathered stats, run the value-level checks.
    for col in &table.columns {
        if let Some(stat) = stats.get(&col.name.value) {
            for check in pending.get(&col.name.value).into_iter().flatten() {
                if let Some(problem) = check.check_data(table, col, stat) {
                    out.push(problem);
                }
            }
        }
    }

    // Uniqueness (D02) compares values by their physical encoding, which is only
    // sound for comparable types (see `site/validation.md`). A column whose type
    // can't be compared is skipped with a D03 warning rather than checked wrongly.
    let barriers = data_dict_parquet::uniqueness_barriers(parquet_path)?;
    let mut uniqueness = Vec::new();
    for col in table
        .columns
        .iter()
        .filter(|col| col.has(Constraint::Unique) && present(&col.name.value))
    {
        if let Some(&reason) = barriers.get(&col.name.value) {
            out.push(uniqueness_not_verified_column(table, col, reason));
            continue;
        }
        let Some(meta) = metadata.get(&col.name.value) else {
            continue;
        };
        match crate::validate_meta::validate_d02_unique_column(table, col, meta) {
            CheckResult::Pass => {}
            CheckResult::Inconclusive => uniqueness.push(UniquenessTarget::Column(col)),
            CheckResult::Fail(problem) => out.push(*problem),
        }
    }
    let primary_key = table
        .columns
        .iter()
        .filter(|col| col.has(Constraint::PrimaryKey))
        .collect::<Vec<_>>();
    if !primary_key.is_empty() && primary_key.iter().all(|col| present(&col.name.value)) {
        let barrier = primary_key
            .iter()
            .find_map(|col| barriers.get(&col.name.value).map(|&reason| (col, reason)));
        match barrier {
            Some((col, reason)) => {
                out.push(uniqueness_not_verified_primary_key(
                    table,
                    &primary_key,
                    &col.name.value,
                    reason,
                ));
            }
            None => uniqueness.push(UniquenessTarget::PrimaryKey(primary_key)),
        }
    }
    if !uniqueness.is_empty() {
        let checks = uniqueness
            .iter()
            .map(UniquenessTarget::check)
            .collect::<Vec<_>>();
        let results = data_dict_parquet::uniqueness_stats(parquet_path, &checks, SAMPLE_LIMIT)?;
        for (target, stats) in uniqueness.iter().zip(&results) {
            if stats.duplicate_count == 0 {
                continue;
            }
            match target {
                UniquenessTarget::Column(col) => {
                    out.push(duplicates_in_unique_column(table, col, stats));
                }
                UniquenessTarget::PrimaryKey(columns) => {
                    out.push(duplicates_in_primary_key(table, columns, stats));
                }
            }
        }
    }

    Ok(())
}

/// A value-level column check, split into the data it needs and the verdict it
/// draws from that data. Keeping the two together (rather than in the
/// orchestrator) lets the scanner compute the union of all checks' needs in a
/// single pass, and lets a new check be added without touching the pipeline.
trait ColumnCheck {
    /// Attempt the check from footer metadata alone.
    fn check_meta(&self, table: &Table, col: &Column, meta: &ColumnMeta) -> CheckResult;

    /// What this check needs read from the column's data. `actual` is the
    /// column's data-side type (one of the six dictionary type names), letting
    /// a check opt out when the data can't support it. Returning the default
    /// (nothing requested) opts the column out of this check.
    fn needs(&self, col: &Column, actual: &str) -> ColumnNeeds;

    /// Draw a verdict from the gathered stats. Only ever called with stats whose
    /// requested fields this check (or another) asked for. `table` is passed for
    /// locating the finding at the column's node in the dictionary.
    /// Complete an inconclusive metadata check from scanned values. `None` is
    /// pass and `Some` is fail; data checks cannot remain inconclusive.
    fn check_data(&self, table: &Table, col: &Column, stats: &ColumnStats) -> Option<Problem>;
}

/// Every value-level check, run against each present column. Add a check here
/// and the plan/scan/check pipeline picks it up automatically.
const VALUE_CHECKS: &[&dyn ColumnCheck] = &[&RequiredNotNull, &EnumMembership];

/// D01 — a `required` (or `primary_key`) column must contain no nulls.
struct RequiredNotNull;

impl ColumnCheck for RequiredNotNull {
    fn check_meta(&self, table: &Table, col: &Column, meta: &ColumnMeta) -> CheckResult {
        crate::validate_meta::validate_d01_required_not_null(table, col, meta)
    }

    fn needs(&self, col: &Column, _actual: &str) -> ColumnNeeds {
        ColumnNeeds {
            nulls: col.is_required_implied(),
            ..ColumnNeeds::default()
        }
    }

    fn check_data(&self, table: &Table, col: &Column, stats: &ColumnStats) -> Option<Problem> {
        // Nulls are only counted when this check requested them (i.e. the column
        // is required), so a positive count is exactly a violation.
        if stats.null_count == 0 {
            return None;
        }
        Some(nulls_in_required_data(
            table,
            col,
            stats.null_count,
            stats.null_rows.clone(),
        ))
    }
}

/// D04 — an `enum` column's values must all be among its declared `values`.
struct EnumMembership;

impl ColumnCheck for EnumMembership {
    fn check_meta(&self, _table: &Table, col: &Column, _meta: &ColumnMeta) -> CheckResult {
        crate::validate_meta::validate_d04_enum_membership(col)
    }

    fn needs(&self, col: &Column, actual: &str) -> ColumnNeeds {
        // Membership is string equality on a string-like column; a numeric
        // backing is already an M01, so its values are not scanned.
        ColumnNeeds {
            allowed: matches!(actual, "string" | "enum")
                .then(|| enum_allowed(col))
                .flatten(),
            ..ColumnNeeds::default()
        }
    }

    fn check_data(&self, table: &Table, col: &Column, stats: &ColumnStats) -> Option<Problem> {
        // The set was only requested for enum columns, so any outside value is a
        // violation.
        if stats.outside_count == 0 {
            return None;
        }
        Some(values_outside_enum(table, col, stats))
    }
}

/// An `enum` column's allowed values, or `None` when the column declares no
/// `values` (so it opts out of the check). Membership is plain string equality
/// against the string-like column the metadata level guarantees (M01).
fn enum_allowed(col: &Column) -> Option<HashSet<String>> {
    let values = col.values.as_ref()?;
    Some(
        values
            .items
            .iter()
            .filter_map(|item| item.value.as_enum_value().map(str::to_owned))
            .collect(),
    )
}

fn values_outside_enum(table: &Table, col: &Column, stats: &ColumnStats) -> Problem {
    let count = stats.outside_count;
    let rows = crate::problem::format_rows(&stats.outside_rows, count);
    let plural = if count == 1 { "" } else { "s" };
    let sample = stats
        .outside_values
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let values_span = col
        .values
        .as_ref()
        .map_or_else(|| col.name.span.clone(), |values| values.span.clone());
    Problem {
        code: Some("D04"),
        severity: Severity::Error,
        message: format!("has {count} value{plural} outside the allowed set ({sample}; {rows})"),
        column: None,
        expected: Some("An enum column's values must all be among its declared `values`.".into()),
        hint: None,
        suggestion: None,
        context: vec![table.name.span.clone(), col.name.span.clone(), values_span],
        kind: ProblemKind::ValuesOutsideEnum {
            count,
            rows: stats.outside_rows.clone(),
            values: stats.outside_values.clone(),
        },
    }
}

fn nulls_in_required_data(table: &Table, col: &Column, count: usize, rows: Vec<usize>) -> Problem {
    let detail = crate::problem::format_rows(&rows, count);
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
        message: format!("has {count} null value{plural} ({detail})"),
        column: None,
        expected: Some("A required column must not contain nulls.".into()),
        hint: None,
        suggestion: None,
        context: vec![
            table.name.span.clone(),
            col.name.span.clone(),
            constraint_span,
        ],
        kind: ProblemKind::NullsInRequired { count, rows },
    }
}

fn duplicates_in_unique_column(table: &Table, col: &Column, stats: &UniquenessStats) -> Problem {
    let count = stats.duplicate_count;
    let detail = crate::problem::format_rows(&stats.duplicate_rows, count);
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
        message: format!("has {count} repeated occurrence{plural} ({detail})"),
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
            rows: stats.duplicate_rows.clone(),
        },
    }
}

fn duplicates_in_primary_key(
    table: &Table,
    columns: &[&Column],
    stats: &UniquenessStats,
) -> Problem {
    let count = stats.duplicate_count;
    let detail = crate::problem::format_rows(&stats.duplicate_rows, count);
    let plural = if count == 1 { "" } else { "s" };
    let last = columns
        .last()
        .expect("a primary key has at least one column");
    let constraint_span = last
        .constraints
        .iter()
        .find(|constraint| constraint.value == Constraint::PrimaryKey)
        .map_or_else(
            || last.name.span.clone(),
            |constraint| constraint.span.clone(),
        );
    Problem {
        code: Some("D02"),
        severity: Severity::Error,
        message: format!("has {count} repeated occurrence{plural} ({detail})"),
        column: None,
        expected: Some("The primary key must uniquely identify every row.".into()),
        hint: None,
        suggestion: None,
        context: std::iter::once(table.name.span.clone())
            .chain(columns.iter().map(|col| col.name.span.clone()))
            .chain(std::iter::once(constraint_span))
            .collect(),
        kind: ProblemKind::DuplicateValues {
            columns: columns.iter().map(|col| col.name.value.clone()).collect(),
            count,
            rows: stats.duplicate_rows.clone(),
        },
    }
}

/// A human phrase for a uniqueness barrier slug (see
/// `data_dict_parquet::uniqueness_barriers`), used in the D03 message.
fn barrier_phrase(reason: &str) -> &'static str {
    match reason {
        "json" => "JSON",
        "bson" => "BSON",
        "nested" => "a nested type",
        _ => "an unrecognized type",
    }
}

fn uniqueness_not_verified_column(table: &Table, col: &Column, reason: &str) -> Problem {
    let constraint_span = col
        .constraints
        .iter()
        .find(|constraint| constraint.value == Constraint::Unique)
        .map_or_else(
            || col.name.span.clone(),
            |constraint| constraint.span.clone(),
        );
    Problem {
        code: Some("D03"),
        severity: Severity::Warning,
        message: format!(
            "`{}` has {}, whose values can't be compared for uniqueness",
            col.name.value,
            barrier_phrase(reason)
        ),
        column: None,
        expected: Some("Uniqueness can only be verified for comparable types.".into()),
        hint: None,
        suggestion: None,
        context: vec![
            table.name.span.clone(),
            col.name.span.clone(),
            constraint_span,
        ],
        kind: ProblemKind::UniquenessNotVerified {
            columns: vec![col.name.value.clone()],
            reason: reason.to_string(),
        },
    }
}

fn uniqueness_not_verified_primary_key(
    table: &Table,
    columns: &[&Column],
    barrier: &str,
    reason: &str,
) -> Problem {
    let last = columns
        .last()
        .expect("a primary key has at least one column");
    let constraint_span = last
        .constraints
        .iter()
        .find(|constraint| constraint.value == Constraint::PrimaryKey)
        .map_or_else(
            || last.name.span.clone(),
            |constraint| constraint.span.clone(),
        );
    Problem {
        code: Some("D03"),
        severity: Severity::Warning,
        message: format!(
            "primary key column `{}` has {}, whose values can't be compared for uniqueness",
            barrier,
            barrier_phrase(reason)
        ),
        column: None,
        expected: Some("Uniqueness can only be verified for comparable types.".into()),
        hint: None,
        suggestion: None,
        context: std::iter::once(table.name.span.clone())
            .chain(columns.iter().map(|col| col.name.span.clone()))
            .chain(std::iter::once(constraint_span))
            .collect(),
        kind: ProblemKind::UniquenessNotVerified {
            columns: columns.iter().map(|col| col.name.value.clone()).collect(),
            reason: reason.to_string(),
        },
    }
}

/// D05/D06 — referential integrity. Runs once over the tables that were read,
/// checking each single-column foreign key's values against the `primary_key` it
/// references, whose data may live in another table's source.
fn foreign_key_issues(dict: &DataDict, readable: &ReadTables, out: &mut ProblemSet) {
    let mut checks = Vec::new();
    let mut targets = Vec::new();
    for table in &dict.tables {
        let Some((child_path, child_columns)) = readable.get(&table.name.value) else {
            continue;
        };
        for col in &table.columns {
            // A foreign key column absent from the data is already an M02; don't
            // also fail its data read here.
            if !col.has(Constraint::ForeignKey) || !child_columns.contains(&col.name.value) {
                continue;
            }
            let Some((parent_table, parent_col)) = dict.resolve_foreign_key(table, col) else {
                continue;
            };
            let Some((parent_path, parent_columns)) = readable.get(&parent_table.name.value) else {
                continue;
            };
            if !parent_columns.contains(&parent_col.name.value) {
                continue;
            }
            checks.push(ForeignKeyCheck {
                child_path: child_path.clone(),
                child_column: col.name.value.clone(),
                parent_path: parent_path.clone(),
                parent_column: parent_col.name.value.clone(),
            });
            targets.push((table, col, parent_table, parent_col));
        }
    }
    if checks.is_empty() {
        return;
    }
    let results = match data_dict_parquet::foreign_key_stats(&checks, SAMPLE_LIMIT) {
        Ok(results) => results,
        Err(e) => {
            out.push(Problem::preflight(ProblemKind::Parquet, e.to_string()));
            return;
        }
    };
    for ((table, col, parent_table, parent_col), result) in targets.iter().zip(results) {
        match result {
            ForeignKeyResult::NotVerified { reason } => out.push(
                referential_integrity_not_verified(table, col, parent_table, parent_col, reason),
            ),
            ForeignKeyResult::Checked(stats) if stats.orphan_count > 0 => {
                out.push(foreign_key_not_found(
                    table,
                    col,
                    parent_table,
                    parent_col,
                    &stats,
                ));
            }
            ForeignKeyResult::Checked(_) => {}
        }
    }
}

fn fk_constraint_span(col: &Column) -> quarto_source_map::SourceInfo {
    col.constraints
        .iter()
        .find(|constraint| constraint.value == Constraint::ForeignKey)
        .map_or_else(
            || col.name.span.clone(),
            |constraint| constraint.span.clone(),
        )
}

fn foreign_key_not_found(
    table: &Table,
    col: &Column,
    parent_table: &Table,
    parent_col: &Column,
    stats: &ForeignKeyStats,
) -> Problem {
    let count = stats.orphan_count;
    let detail = crate::problem::format_rows(&stats.orphan_rows, count);
    let plural = if count == 1 { "" } else { "s" };
    let sample = stats
        .orphan_values
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let references = format!("{}.{}", parent_table.name.value, parent_col.name.value);
    Problem {
        code: Some("D05"),
        severity: Severity::Error,
        message: format!(
            "has {count} value{plural} not found in `{references}` ({sample}; {detail})"
        ),
        column: None,
        expected: Some(
            "A foreign key's values must all appear in the primary key it references.".into(),
        ),
        hint: None,
        suggestion: None,
        context: vec![
            table.name.span.clone(),
            col.name.span.clone(),
            fk_constraint_span(col),
        ],
        kind: ProblemKind::ForeignKeyNotFound {
            column: col.name.value.clone(),
            references,
            count,
            rows: stats.orphan_rows.clone(),
            values: stats.orphan_values.clone(),
        },
    }
}

fn referential_integrity_not_verified(
    table: &Table,
    col: &Column,
    parent_table: &Table,
    parent_col: &Column,
    reason: &str,
) -> Problem {
    let references = format!("{}.{}", parent_table.name.value, parent_col.name.value);
    Problem {
        code: Some("D06"),
        severity: Severity::Warning,
        message: format!(
            "can't be verified against `{references}`: {} values aren't comparable",
            barrier_phrase(reason)
        ),
        column: None,
        expected: Some("Referential integrity can only be verified for comparable types.".into()),
        hint: None,
        suggestion: None,
        context: vec![
            table.name.span.clone(),
            col.name.span.clone(),
            fk_constraint_span(col),
        ],
        kind: ProblemKind::ReferentialIntegrityNotVerified {
            column: col.name.value.clone(),
            references,
            reason: reason.to_string(),
        },
    }
}

enum UniquenessTarget<'a> {
    Column(&'a Column),
    PrimaryKey(Vec<&'a Column>),
}

impl UniquenessTarget<'_> {
    fn check(&self) -> UniquenessCheck {
        let columns = match self {
            UniquenessTarget::Column(col) => vec![col.name.value.clone()],
            UniquenessTarget::PrimaryKey(columns) => {
                columns.iter().map(|col| col.name.value.clone()).collect()
            }
        };
        UniquenessCheck { columns }
    }
}
