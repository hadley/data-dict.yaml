//! Data-level validation, the `D##` checks (see `site/validation.md`).
//!
//! [`validate_data`] is the entry point; `value_issues` is the value-checking
//! core it runs after the metadata checks ([`crate::validate_meta`]).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use data_dict_parquet::{
    ColumnMeta, ColumnNeeds, ColumnRequest, ColumnStats, DataColumn, ForeignKeyCheck,
    ForeignKeyResult, ForeignKeyStats, UniquenessCheck, UniquenessStats,
};

use chrono::{DateTime, Utc};
use quarto_source_map::SourceInfo;

use crate::model::{Assertion, Column, Constraint, DataDict, Table};
use crate::problem::{FailedTableRows, Problem, ProblemKind, ProblemSet, Severity, ValueRow};
use crate::report::{Failed, StepKey, StepTarget, in_dictionary_order, table_assertions};
use crate::validate_meta::CheckResult;
use crate::{Level, ReadTables};

/// How many example values (e.g. offending rows) to record per validation
/// issue. Issues count every offender but only list this many. The report
/// browses the whole sample; a terminal diagnostic lists only the first few
/// (see [`crate::problem::LIST_LIMIT`]).
const SAMPLE_LIMIT: usize = 50;

/// A `display: restricted` column's values never appear in diagnostics.
fn is_restricted(col: &Column) -> bool {
    col.display.as_deref() == Some("restricted")
}

/// Pair each sampled row's values with the columns they came from, leaving out
/// every column `restricted` names. Withholding is per column, so a key's
/// unrestricted columns still report their values; a row left naming nothing at
/// all is dropped, since an empty entry says nothing.
///
/// Returns the rows and whether anything was withheld, which the problem
/// reports as `redacted`.
fn value_rows(
    columns: &[String],
    values: &[Vec<String>],
    restricted: impl Fn(&str) -> bool,
) -> (Vec<ValueRow>, bool) {
    let kept: Vec<bool> = columns.iter().map(|name| !restricted(name)).collect();
    let redacted = kept.iter().any(|keep| !keep);
    let rows = values
        .iter()
        .map(|value| {
            columns
                .iter()
                .zip(value)
                .zip(&kept)
                .filter(|(_, keep)| **keep)
                .map(|((name, value), _)| (name.clone(), Some(value.clone())))
                .collect::<ValueRow>()
        })
        .filter(|row| !row.is_empty())
        .collect();
    (rows, redacted)
}

/// Withhold the values of every restricted column an assertion reads, dropping
/// a row left naming nothing. An assertion names its columns by path, and only
/// a whole column can be restricted, so the root segment decides.
fn restrict_assertion_values(table: &Table, values: Vec<ValueRow>) -> (Vec<ValueRow>, bool) {
    let restricted = |path: &str| {
        let root = path.split('.').next().unwrap_or(path);
        table
            .columns
            .iter()
            .any(|col| col.name.value == root && is_restricted(col))
    };
    let mut redacted = false;
    let mut kept = Vec::new();
    for mut row in values {
        redacted |= row.retain(|column| !restricted(column));
        if !row.is_empty() {
            kept.push(row);
        }
    }
    (kept, redacted)
}

/// Record that a check found nothing. A check whose column declares nothing for
/// it has no step to record against.
fn record_pass(out: &mut ProblemSet, step: Option<&StepKey>) {
    if let Some(step) = step {
        out.step_pass(step);
    }
}

/// Record a check's finding against its step, weighing the step by the rows the
/// finding blames.
fn record_fail(out: &mut ProblemSet, step: Option<&StepKey>, problem: Problem) {
    match step {
        Some(step) => out.push_for(step, failed_rows(&problem), problem),
        None => out.push(problem),
    }
}

/// How many rows a problem blames. A finding with a single verdict about the
/// whole table blames every row of it, so it can be counted alongside the
/// row-level ones (see `site/report.md`).
fn failed_rows(problem: &Problem) -> Failed {
    match &problem.kind {
        ProblemKind::NullsInRequired { count, .. }
        | ProblemKind::DuplicateValues { count, .. }
        | ProblemKind::ValuesOutsideEnum { count, .. }
        | ProblemKind::ForeignKeyNotFound { count, .. }
        | ProblemKind::AssertionViolated { count, .. } => Failed::Rows(*count),
        _ => Failed::AllRows,
    }
}

/// The `(values; rows)` tail every value-reporting diagnostic ends with, naming
/// the offending values when they can be shown and saying so when they can't.
fn found(values: &[ValueRow], rows: &[usize], count: usize, redacted: bool) -> String {
    let listed = crate::problem::format_rows(rows, count);
    match crate::problem::format_values(values) {
        Some(values) => format!("{values}; {listed}"),
        None if redacted => format!("values restricted; {listed}"),
        None => listed,
    }
}

/// Validate a parquet file's values against a data dictionary.
///
/// Validates the spec first, then — when it is free of errors — runs every
/// metadata-level check ([`crate::validate_meta`]) plus the value-level checks
/// below: reading the columns and pages the checks imply and reporting, for
/// example, nulls in a required column.
pub fn validate_data(dict_path: &Path, table: Option<&str>) -> ProblemSet {
    // One reading of the clock for the whole run, so every `NOW()` in every
    // assertion of every table agrees; see `site/expression-execution.md`.
    let now = Utc::now();
    crate::compare_dataset(
        dict_path,
        table,
        Level::Data,
        |table, parquet_path, actual, problems| {
            crate::validate_meta::meta_issues(table, actual, problems);
            if let Err(e) = value_issues(table, parquet_path, actual, problems) {
                problems.push(Problem::preflight(ProblemKind::Parquet, e.to_string()));
            }
            if let Err(e) = assertion_issues(table, parquet_path, actual, now, problems) {
                problems.push(Problem::preflight(ProblemKind::Parquet, e.to_string()));
            }
            if let Err(e) = attach_primary_keys(
                table,
                |name| actual.iter().any(|c| c.name == name),
                parquet_path,
                problems,
            ) {
                problems.push(Problem::preflight(ProblemKind::Parquet, e.to_string()));
            }
        },
        |dict, readable, problems| {
            foreign_key_issues(dict, readable, problems);
            failed_rows_export(dict, readable, problems);
        },
    )
}

/// Gather the first few failing rows of each table — every declared column,
/// not just the ones a check names — for the report's failed-rows page. The
/// rows are the union of every row the table's problems named, ascending and
/// capped; a restricted column, and a nested one a per-row value can't name,
/// is left out, restriction flagging `redacted`.
fn failed_rows_export(dict: &DataDict, readable: &ReadTables, out: &mut ProblemSet) {
    for table in &dict.tables {
        let Some((path, columns)) = readable.get(&table.name.value) else {
            continue;
        };
        let mut rows: Vec<usize> = out
            .items
            .iter()
            .filter(|p| p.table.as_deref() == Some(table.name.value.as_str()))
            .filter_map(|p| sample_rows(&p.kind))
            .flatten()
            .copied()
            .collect();
        rows.sort_unstable();
        rows.dedup();
        let count = rows.len();
        rows.truncate(SAMPLE_LIMIT);
        if rows.is_empty() {
            continue;
        }
        let restricted = |name: &str| {
            table
                .columns
                .iter()
                .any(|c| c.name.value == name && is_restricted(c))
        };
        let redacted = columns.iter().any(|name| restricted(name));
        let names: Vec<String> = table
            .columns
            .iter()
            .filter(|c| c.fields.is_none())
            .map(|c| c.name.value.clone())
            .filter(|name| columns.contains(name) && !restricted(name))
            .collect();
        let key: Vec<bool> = names
            .iter()
            .map(|name| {
                table
                    .columns
                    .iter()
                    .any(|c| c.name.value == *name && c.has(Constraint::PrimaryKey))
            })
            .collect();
        let zero_based: Vec<usize> = rows.iter().map(|row| row - 1).collect();
        match data_dict_parquet::values_at_rows(path, &names, &zero_based) {
            Ok(fetched) => {
                // No key columns left to name: an empty list, so the key is
                // omitted rather than serialized as a list of empty objects.
                let any_key = key.iter().any(|&is| is);
                let mut keys = Vec::new();
                let mut values = Vec::new();
                for value in fetched {
                    let entry: Vec<_> = names.iter().cloned().zip(value).collect();
                    if any_key {
                        keys.push(
                            entry
                                .iter()
                                .enumerate()
                                .filter(|(j, _)| key[*j])
                                .map(|(_, e)| e.clone())
                                .collect(),
                        );
                    }
                    values.push(
                        entry
                            .into_iter()
                            .enumerate()
                            .filter(|(j, _)| !key[*j])
                            .map(|(_, e)| e)
                            .collect(),
                    );
                }
                out.failed_rows.push(FailedTableRows {
                    table: table.name.value.clone(),
                    count,
                    rows,
                    keys,
                    values,
                    redacted,
                });
            }
            Err(e) => out.push(Problem::preflight(ProblemKind::Parquet, e.to_string())),
        }
    }
}

/// Run the value-level checks for the dictionary's `table` against the data,
/// pushing any problems found into `out`. `actual` is the column schema already
/// read for the metadata checks, used here only to tell which columns are
/// present.
fn value_issues(
    table: &Table,
    parquet_path: &Path,
    actual: &[DataColumn],
    out: &mut ProblemSet,
) -> Result<(), data_dict_parquet::ParquetError> {
    let present = |name: &str| actual.iter().any(|c| c.name == name);
    let metadata = data_dict_parquet::column_meta(parquet_path)?;

    // Phase 1 — check the footer. A data-level rule remains D## even when
    // Parquet metadata is sufficient to prove its result. Only inconclusive
    // checks are allowed to request a value scan.
    let mut plan: Vec<(ColumnRequest, &Column, Vec<&dyn ColumnCheck>)> = Vec::new();
    for col in &table.columns {
        let Some(data) = actual.iter().find(|c| c.name == col.name.value) else {
            continue;
        };
        let Some(meta) = metadata.get(&col.name.value) else {
            continue;
        };
        let mut merged = ColumnNeeds::default();
        let mut pending: Vec<&dyn ColumnCheck> = Vec::new();
        let path = vec![col.name.value.clone()];
        for check in VALUE_CHECKS {
            let step = check.step(table, col, &path);
            match check.check_meta(table, col, meta) {
                CheckResult::Pass => record_pass(out, step.as_ref()),
                CheckResult::Inconclusive => {
                    merged = merged.merge(check.needs(col, &data.dict_type));
                    pending.push(*check);
                }
                CheckResult::Fail(problem) => record_fail(out, step.as_ref(), *problem),
            }
        }
        if merged.any() {
            plan.push((
                ColumnRequest {
                    path: vec![col.name.value.clone()],
                    needs: merged,
                },
                col,
                pending,
            ));
        }
        // Fields carry no constraints, so of the value checks only enum
        // membership (D04) applies below the top level; register it for
        // every enum field reachable through structs (and their lists).
        if let Some(fields) = &col.fields {
            plan_enum_fields(fields, data, &mut vec![col.name.value.clone()], &mut plan);
        }
    }

    // Phase 2 — scan. Gather exactly those statistics, in one pass, reading only
    // the columns and pages the plan implies.
    let requests: Vec<ColumnRequest> = plan
        .iter()
        .map(|(request, _, _)| ColumnRequest {
            path: request.path.clone(),
            needs: request.needs.clone(),
        })
        .collect();
    let stats = data_dict_parquet::column_stats(parquet_path, &requests, SAMPLE_LIMIT)?;

    // Phase 3 — check. Per planned column, draw verdicts from the gathered stats.
    for ((request, col, pending), stat) in plan.iter().zip(&stats) {
        for check in pending {
            let step = check.step(table, col, &request.path);
            match check.check_data(table, col, &request.path, stat) {
                Some(problem) => match step {
                    Some(step) => out.push_for(&step, check.failed(stat), problem),
                    None => out.push(problem),
                },
                None => record_pass(out, step.as_ref()),
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
        let step = StepKey::new(
            &table.name.value,
            StepTarget::Unique(col.name.value.clone()),
        );
        if let Some(&reason) = barriers.get(&col.name.value) {
            out.push_at(&step, uniqueness_not_verified_column(table, col, reason));
            continue;
        }
        let Some(meta) = metadata.get(&col.name.value) else {
            continue;
        };
        match crate::validate_meta::validate_d02_unique_column(table, col, meta) {
            CheckResult::Pass => out.step_pass(&step),
            CheckResult::Inconclusive => uniqueness.push(UniquenessTarget::Column(col)),
            CheckResult::Fail(problem) => record_fail(out, Some(&step), *problem),
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
                out.push_at(
                    &StepKey::new(&table.name.value, StepTarget::PrimaryKey),
                    uniqueness_not_verified_primary_key(
                        table,
                        &primary_key,
                        &col.name.value,
                        reason,
                    ),
                );
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
            let step = match target {
                UniquenessTarget::Column(col) => StepKey::new(
                    &table.name.value,
                    StepTarget::Unique(col.name.value.clone()),
                ),
                UniquenessTarget::PrimaryKey(_) => {
                    StepKey::new(&table.name.value, StepTarget::PrimaryKey)
                }
            };
            if stats.duplicate_count == 0 {
                out.step_pass(&step);
                continue;
            }
            let problem = match target {
                UniquenessTarget::Column(col) => duplicates_in_unique_column(table, col, stats),
                UniquenessTarget::PrimaryKey(columns) => {
                    duplicates_in_primary_key(table, columns, stats)
                }
            };
            record_fail(out, Some(&step), problem);
        }
    }

    Ok(())
}

/// Register a D04 request for every enum field under `fields` (recursively,
/// through nested structs and their lists), pairing each with the
/// [`EnumMembership`] check so phase 3 reports through the field's own node.
/// `path` holds the segments down to the enclosing column/field.
fn plan_enum_fields<'a>(
    fields: &'a [Column],
    data: &data_dict_parquet::DataColumn,
    path: &mut Vec<String>,
    plan: &mut Vec<(ColumnRequest, &'a Column, Vec<&'static dyn ColumnCheck>)>,
) {
    for field in fields {
        let Some(child) = data.children.iter().find(|c| c.name == field.name.value) else {
            continue;
        };
        path.push(field.name.value.clone());
        if matches!(
            crate::validate_meta::validate_d04_enum_membership(field),
            CheckResult::Inconclusive
        ) {
            let needs = EnumMembership.needs(field, &child.dict_type);
            if needs.any() {
                plan.push((
                    ColumnRequest {
                        path: path.clone(),
                        needs,
                    },
                    field,
                    vec![&EnumMembership as &dyn ColumnCheck],
                ));
            }
        }
        if let Some(nested) = &field.fields {
            plan_enum_fields(nested, child, path, plan);
        }
        path.pop();
    }
}

/// Each of the table's `assert` expressions with the columns it reads, in
/// dictionary order — what a `D07` step is registered from. An expression that
/// can't be lowered reads no column as far as the report is concerned; the
/// spec level has already reported why.
pub(crate) fn assertion_targets(table: &Table) -> Vec<(String, Vec<String>)> {
    let env = crate::validate_spec::TableEnv::new(table);
    let defs = crate::validate_spec::definition_exprs(table);
    table_assertions(table)
        .into_iter()
        .map(|(assertion, _)| {
            let columns = assertion
                .expr
                .as_ref()
                .map(|expr| crate::assert_expr::substitute_definitions(expr, &defs))
                .and_then(|expr| crate::assert_expr::lower(&expr, &env))
                .map(|ir| {
                    crate::eval::column_requests(&ir)
                        .iter()
                        .map(|request| request.path.join("."))
                        .collect()
                })
                .unwrap_or_default();
            let mut columns: Vec<String> = columns;
            in_dictionary_order(table, &mut columns);
            (assertion.text.value.clone(), columns)
        })
        .collect()
}

/// Evaluate the table's `assert` expressions against its data (D07–D09).
///
/// Assertions don't join [`VALUE_CHECKS`]: that pipeline is per column, and an
/// assertion reads several and may belong to the table rather than any one of
/// them. `now` is bound once per run by the caller, so every assertion in the
/// run agrees about the current time.
fn assertion_issues(
    table: &Table,
    parquet_path: &Path,
    actual: &[DataColumn],
    now: DateTime<Utc>,
    out: &mut ProblemSet,
) -> Result<(), data_dict_parquet::ParquetError> {
    let env = crate::validate_spec::TableEnv::new(table);
    let defs = crate::validate_spec::definition_exprs(table);

    for (position, (assertion, col)) in table_assertions(table).into_iter().enumerate() {
        // An assertion abandoned below leaves its step unevaluated: nothing
        // compared it against the data.
        let step = StepKey::new(&table.name.value, StepTarget::Assertion(position));
        // An expression that failed to parse or check was already reported at
        // the spec level, and never reaches a verdict here.
        let Some(expr) = &assertion.expr else {
            continue;
        };
        // A reference to a definition must become the definition's expression
        // to evaluate: the data has no such column.
        let expr = crate::assert_expr::substitute_definitions(expr, &defs);
        let Some(ir) = crate::assert_expr::lower(&expr, &env) else {
            continue;
        };

        // A column the data doesn't have is already M02; reporting it again
        // here would say the same thing twice.
        let requests = crate::eval::column_requests(&ir);
        if requests
            .iter()
            .any(|r| !actual.iter().any(|c| c.name == r.path[0]))
        {
            continue;
        }
        let mut columns: Vec<String> = requests.iter().map(|r| r.path.join(".")).collect();
        in_dictionary_order(table, &mut columns);

        // Ask whether every column can be read as its declared type before
        // reading anything: an assertion that can't run is D08.
        let verdicts = data_dict_parquet::decodable(parquet_path, &requests)?;
        if let Some((request, reason)) = requests.iter().zip(&verdicts).find_map(|(r, v)| match v {
            data_dict_parquet::Decodable::No(reason) => Some((r, *reason)),
            data_dict_parquet::Decodable::Yes => None,
        }) {
            out.push_at(
                &step,
                assertion_not_checked(
                    table,
                    col,
                    assertion,
                    &columns,
                    Some(&request.path.join(".")),
                    reason,
                ),
            );
            continue;
        }

        let outcome = crate::eval::evaluate(parquet_path, &ir, now, SAMPLE_LIMIT)?;
        match assertion_problem(table, col, assertion, &columns, outcome) {
            // An assertion that overflowed (D09) or whose pattern was invalid
            // (D08) never reached a verdict, so its step stays unevaluated.
            Some(problem) if !problem.kind.evaluated() => out.push_at(&step, problem),
            Some(problem) => record_fail(out, Some(&step), problem),
            None => out.step_pass(&step),
        }
    }
    Ok(())
}

/// Turn one assertion's outcome into a problem, or `None` when it held.
fn assertion_problem(
    table: &Table,
    col: Option<&Column>,
    assertion: &Assertion,
    columns: &[String],
    outcome: crate::eval::Outcome,
) -> Option<Problem> {
    let text = assertion.text.value.clone();
    let (message, expected, kind) = match outcome {
        crate::eval::Outcome::Rows { count: 0, .. } => return None,
        crate::eval::Outcome::Table { holds: true } => return None,
        crate::eval::Outcome::Rows {
            count,
            rows,
            values,
        } => {
            let (values, redacted) = restrict_assertion_values(table, values);
            let plural = if count == 1 { "" } else { "s" };
            let listed = list_rows(&rows, count);
            let sample = values
                .first()
                .map(|row| row.pairs().collect::<Vec<_>>().join(", "))
                .or_else(|| redacted.then(|| "values restricted".to_string()))
                .map_or(String::new(), |sample| format!(" ({sample})"));
            (
                format!("is false for {count} row{plural}: {listed}{sample}"),
                "An assertion must hold for every row.",
                ProblemKind::AssertionViolated {
                    assertion: text,
                    count,
                    rows,
                    keys: Vec::new(),
                    values,
                    redacted,
                },
            )
        }
        crate::eval::Outcome::Table { holds: false } => (
            "is false for this table".to_string(),
            "An aggregate assertion must hold for the table.",
            ProblemKind::AssertionFalse { assertion: text },
        ),
        crate::eval::Outcome::Faulted { fault, row } => {
            let where_ = row.map_or_else(String::new, |r| format!(" at row {r}"));
            match fault {
                crate::eval::Fault::BadPattern(pattern) => {
                    return Some(assertion_not_checked(
                        table,
                        col,
                        assertion,
                        columns,
                        None,
                        &format!("`{pattern}`{where_} is not a valid regular expression"),
                    ));
                }
                crate::eval::Fault::Overflow => (
                    format!("overflows a 64-bit integer{where_}"),
                    "An assertion's arithmetic must stay within 64-bit integers.",
                    ProblemKind::AssertionOverflow {
                        assertion: text,
                        row,
                    },
                ),
            }
        }
    };
    Some(Problem {
        code: kind.code(),
        step: None,
        severity: Severity::Error,
        message,
        table: Some(table.name.value.clone()),
        columns: columns.to_vec(),
        expected: Some(expected.to_string()),
        hint: None,
        suggestion: None,
        context: assertion_context(table, col, assertion),
        kind,
    })
}

fn assertion_not_checked(
    table: &Table,
    col: Option<&Column>,
    assertion: &Assertion,
    columns: &[String],
    column: Option<&str>,
    reason: &str,
) -> Problem {
    let (message, hint) = match column {
        Some(name) => (
            format!("cannot read `{name}`: {reason}"),
            "Correct the column's declared `type`, or drop the assertion until the data can \
             support it.",
        ),
        None => (
            reason.to_string(),
            "Correct the data the pattern comes from, or write the pattern as a literal so it \
             is checked when the dictionary is validated.",
        ),
    };
    let kind = ProblemKind::AssertionNotChecked {
        assertion: assertion.text.value.clone(),
        column: column.map(str::to_string),
        reason: reason.to_string(),
    };
    Problem {
        code: kind.code(),
        step: None,
        severity: Severity::Error,
        message,
        table: Some(table.name.value.clone()),
        columns: columns.to_vec(),
        expected: Some("An assertion must be evaluable against the data.".into()),
        hint: Some(hint.into()),
        suggestion: None,
        context: assertion_context(table, col, assertion),
        kind,
    }
}

/// The offending row numbers, with an ellipsis when more were counted than
/// sampled. Unlike `format_rows` this omits the `row(s):` label, which the
/// surrounding sentence already supplies.
fn list_rows(rows: &[usize], count: usize) -> String {
    let listed = rows
        .iter()
        .take(crate::problem::LIST_LIMIT)
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if count > rows.len().min(crate::problem::LIST_LIMIT) {
        format!("{listed}, …")
    } else {
        listed
    }
}

/// The table, the column for a column-level assertion, and the `assert` text
/// itself as the highlight. The author's `description` line rides along as
/// context — not an enclosing node, but part of the declaration, so the
/// excerpt shows it whole.
pub(crate) fn assertion_context(
    table: &Table,
    col: Option<&Column>,
    assertion: &Assertion,
) -> Vec<SourceInfo> {
    let mut spans = vec![table.name.span.clone()];
    if let Some(col) = col {
        spans.push(col.name.span.clone());
    }
    if let Some(description) = &assertion.description {
        spans.push(description.span.clone());
    }
    spans.push(assertion.text.span.clone());
    spans
}

/// A value-level column check, split into the data it needs and the verdict it
/// draws from that data. Keeping the two together (rather than in the
/// orchestrator) lets the scanner compute the union of all checks' needs in a
/// single pass, and lets a new check be added without touching the pipeline.
trait ColumnCheck {
    /// Attempt the check from footer metadata alone.
    fn check_meta(&self, table: &Table, col: &Column, meta: &ColumnMeta) -> CheckResult;

    /// The step this check reports through, or `None` when the column declares
    /// nothing for it to check. Must agree with the steps
    /// [`crate::report::Steps::register`] lays out.
    fn step(&self, table: &Table, col: &Column, path: &[String]) -> Option<StepKey>;

    /// How many rows a failure of this check blames, which is not always how
    /// many offending values it counted.
    fn failed(&self, stats: &ColumnStats) -> Failed;

    /// What this check needs read from the column's data. `actual` is the
    /// column's data-side type (one of the six dictionary type names), letting
    /// a check opt out when the data can't support it. Returning the default
    /// (nothing requested) opts the column out of this check.
    fn needs(&self, col: &Column, actual: &str) -> ColumnNeeds;

    /// Draw a verdict from the gathered stats. Only ever called with stats whose
    /// requested fields this check (or another) asked for. `table` is passed for
    /// locating the finding at the column's node in the dictionary, and `path`
    /// names the column as the data holds it — several segments for a field
    /// reached through a struct.
    /// Complete an inconclusive metadata check from scanned values. `None` is
    /// pass and `Some` is fail; data checks cannot remain inconclusive.
    fn check_data(
        &self,
        table: &Table,
        col: &Column,
        path: &[String],
        stats: &ColumnStats,
    ) -> Option<Problem>;
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

    fn step(&self, table: &Table, col: &Column, path: &[String]) -> Option<StepKey> {
        (path.len() == 1 && col.is_required_implied()).then(|| {
            StepKey::new(
                &table.name.value,
                StepTarget::Required(col.name.value.clone()),
            )
        })
    }

    fn failed(&self, stats: &ColumnStats) -> Failed {
        Failed::Rows(stats.null_count)
    }

    fn needs(&self, col: &Column, _actual: &str) -> ColumnNeeds {
        ColumnNeeds {
            nulls: col.is_required_implied(),
            ..ColumnNeeds::default()
        }
    }

    fn check_data(
        &self,
        table: &Table,
        col: &Column,
        _path: &[String],
        stats: &ColumnStats,
    ) -> Option<Problem> {
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

    fn step(&self, table: &Table, col: &Column, path: &[String]) -> Option<StepKey> {
        (col.is_enum() && col.values.is_some())
            .then(|| StepKey::new(&table.name.value, StepTarget::Enum(path.to_vec())))
    }

    // A list column holds several values per row, so the rows that broke the
    // check are fewer than the values that did.
    fn failed(&self, stats: &ColumnStats) -> Failed {
        Failed::Rows(stats.outside_row_count)
    }

    fn needs(&self, col: &Column, actual: &str) -> ColumnNeeds {
        // Membership is string equality on a string-like column; a numeric
        // backing is already an M01, so its values are not scanned. For a
        // list (nested to any depth) the innermost elements are what must be
        // string-like.
        let mut element = actual;
        while let Some(elem) = element
            .strip_prefix("list(")
            .and_then(|s| s.strip_suffix(")"))
        {
            element = elem;
        }
        ColumnNeeds {
            allowed: matches!(element, "string" | "enum")
                .then(|| enum_allowed(col))
                .flatten(),
            ..ColumnNeeds::default()
        }
    }

    fn check_data(
        &self,
        table: &Table,
        col: &Column,
        path: &[String],
        stats: &ColumnStats,
    ) -> Option<Problem> {
        // The set was only requested for enum columns, so any outside value is a
        // violation.
        if stats.outside_count == 0 {
            return None;
        }
        Some(values_outside_enum(table, col, path, stats))
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

fn values_outside_enum(
    table: &Table,
    col: &Column,
    path: &[String],
    stats: &ColumnStats,
) -> Problem {
    let count = stats.outside_count;
    let rowwise: Vec<Vec<String>> = stats
        .outside_values
        .iter()
        .map(|value| vec![value.clone()])
        .collect();
    // The schema allows `display` on a column but not on a nested field, so a
    // field reached through a struct takes its restriction from the column that
    // holds it.
    let restricted = is_restricted(col)
        || path.first().is_some_and(|root| {
            table
                .columns
                .iter()
                .any(|col| col.name.value == *root && is_restricted(col))
        });
    let (values, redacted) = value_rows(&[path.join(".")], &rowwise, |_| restricted);
    let detail = found(&values, &stats.outside_rows, count, redacted);
    let plural = if count == 1 { "" } else { "s" };
    let values_span = col
        .values
        .as_ref()
        .map_or_else(|| col.name.span.clone(), |values| values.span.clone());
    Problem {
        code: Some("D04"),
        step: None,
        severity: Severity::Error,
        message: format!("has {count} value{plural} outside the allowed set ({detail})"),
        table: Some(table.name.value.clone()),
        columns: vec![path.join(".")],
        expected: Some("An enum column's values must all be among its declared `values`.".into()),
        hint: None,
        suggestion: None,
        context: vec![table.name.span.clone(), col.name.span.clone(), values_span],
        kind: ProblemKind::ValuesOutsideEnum {
            count,
            rows: stats.outside_rows.clone(),
            keys: Vec::new(),
            values,
            redacted,
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
        step: None,
        severity: Severity::Error,
        message: format!("has {count} null value{plural} ({detail})"),
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
            rows,
            keys: Vec::new(),
            redacted: false,
        },
    }
}

fn duplicates_in_unique_column(table: &Table, col: &Column, stats: &UniquenessStats) -> Problem {
    let count = stats.duplicate_count;
    let (values, redacted) = value_rows(&stats.key_columns, &stats.duplicate_values, |_| {
        is_restricted(col)
    });
    let detail = found(&values, &stats.duplicate_rows, count, redacted);
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
        message: format!("has {count} repeated occurrence{plural} ({detail})"),
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
            rows: stats.duplicate_rows.clone(),
            keys: Vec::new(),
            values,
            redacted,
        },
    }
}

fn duplicates_in_primary_key(
    table: &Table,
    columns: &[&Column],
    stats: &UniquenessStats,
) -> Problem {
    let count = stats.duplicate_count;
    let (values, redacted) = value_rows(&stats.key_columns, &stats.duplicate_values, |name| {
        columns
            .iter()
            .any(|col| col.name.value == name && is_restricted(col))
    });
    let detail = found(&values, &stats.duplicate_rows, count, redacted);
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
        step: None,
        severity: Severity::Error,
        message: format!("has {count} repeated occurrence{plural} ({detail})"),
        table: Some(table.name.value.clone()),
        columns: columns.iter().map(|col| col.name.value.clone()).collect(),
        expected: Some("The primary key must uniquely identify every row.".into()),
        hint: None,
        suggestion: None,
        context: std::iter::once(table.name.span.clone())
            .chain(columns.iter().map(|col| col.name.span.clone()))
            .chain(std::iter::once(constraint_span))
            .collect(),
        kind: ProblemKind::DuplicateValues {
            count,
            rows: stats.duplicate_rows.clone(),
            keys: Vec::new(),
            values,
            redacted,
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
        step: None,
        severity: Severity::Warning,
        message: format!(
            "`{}` has {}, whose values can't be compared for uniqueness",
            col.name.value,
            barrier_phrase(reason)
        ),
        table: Some(table.name.value.clone()),
        columns: vec![col.name.value.clone()],
        expected: Some("Uniqueness can only be verified for comparable types.".into()),
        hint: None,
        suggestion: None,
        context: vec![
            table.name.span.clone(),
            col.name.span.clone(),
            constraint_span,
        ],
        kind: ProblemKind::UniquenessNotVerified {
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
        step: None,
        severity: Severity::Warning,
        message: format!(
            "primary key column `{}` has {}, whose values can't be compared for uniqueness",
            barrier,
            barrier_phrase(reason)
        ),
        table: Some(table.name.value.clone()),
        columns: columns.iter().map(|col| col.name.value.clone()).collect(),
        expected: Some("Uniqueness can only be verified for comparable types.".into()),
        hint: None,
        suggestion: None,
        context: std::iter::once(table.name.span.clone())
            .chain(columns.iter().map(|col| col.name.span.clone()))
            .chain(std::iter::once(constraint_span))
            .collect(),
        kind: ProblemKind::UniquenessNotVerified {
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
        let step = StepKey::new(
            &table.name.value,
            StepTarget::ForeignKey(col.name.value.clone()),
        );
        match result {
            ForeignKeyResult::NotVerified { reason } => out.push_at(
                &step,
                referential_integrity_not_verified(table, col, parent_table, parent_col, reason),
            ),
            ForeignKeyResult::Checked(stats) if stats.orphan_count > 0 => {
                record_fail(
                    out,
                    Some(&step),
                    foreign_key_not_found(table, col, parent_table, parent_col, &stats),
                );
            }
            ForeignKeyResult::Checked(_) => out.step_pass(&step),
        }
    }
    for table in &dict.tables {
        let Some((path, columns)) = readable.get(&table.name.value) else {
            continue;
        };
        if let Err(e) =
            attach_primary_keys(table, |name| columns.iter().any(|c| c == name), path, out)
        {
            out.push(Problem::preflight(ProblemKind::Parquet, e.to_string()));
        }
    }
}

/// The 1-based row numbers a problem kind names, if it names rows at all.
fn sample_rows(kind: &ProblemKind) -> Option<&Vec<usize>> {
    match kind {
        ProblemKind::NullsInRequired { rows, .. }
        | ProblemKind::DuplicateValues { rows, .. }
        | ProblemKind::ValuesOutsideEnum { rows, .. }
        | ProblemKind::ForeignKeyNotFound { rows, .. }
        | ProblemKind::AssertionViolated { rows, .. } => Some(rows),
        _ => None,
    }
}

/// The row sample a problem kind carries: its 1-based row numbers, the primary
/// keys attached to them, and its withheld-values flag. `None` for a kind that
/// names no rows.
fn sample_mut(kind: &mut ProblemKind) -> Option<(&mut Vec<usize>, &mut Vec<ValueRow>, &mut bool)> {
    match kind {
        ProblemKind::NullsInRequired {
            rows,
            keys,
            redacted,
            ..
        }
        | ProblemKind::DuplicateValues {
            rows,
            keys,
            redacted,
            ..
        }
        | ProblemKind::ValuesOutsideEnum {
            rows,
            keys,
            redacted,
            ..
        }
        | ProblemKind::ForeignKeyNotFound {
            rows,
            keys,
            redacted,
            ..
        }
        | ProblemKind::AssertionViolated {
            rows,
            keys,
            redacted,
            ..
        } => Some((rows, keys, redacted)),
        _ => None,
    }
}

/// Give every row-naming problem of `table` the primary key its listed rows
/// held, so the report can say *which* row failed, not only where it sits. A
/// key column the problem already names (a duplicate primary key reports its
/// own key) is not repeated, and a restricted key column is withheld like any
/// other value, flagging the problem `redacted`.
fn attach_primary_keys(
    table: &Table,
    present: impl Fn(&str) -> bool,
    parquet_path: &Path,
    out: &mut ProblemSet,
) -> Result<(), data_dict_parquet::ParquetError> {
    let keys: Vec<&Column> = table
        .columns
        .iter()
        .filter(|col| col.has(Constraint::PrimaryKey) && present(&col.name.value))
        .collect();
    let withheld = keys.iter().any(|col| is_restricted(col));
    let names: Vec<String> = keys
        .iter()
        .filter(|col| !is_restricted(col))
        .map(|col| col.name.value.clone())
        .collect();
    if names.is_empty() {
        // Every key column is restricted: there is nothing to attach, but the
        // withholding itself is still reported.
        if withheld {
            for problem in &mut out.items {
                if problem.table.as_deref() != Some(table.name.value.as_str()) {
                    continue;
                }
                if let Some((rows, _, redacted)) = sample_mut(&mut problem.kind)
                    && !rows.is_empty()
                {
                    *redacted = true;
                }
            }
        }
        return Ok(());
    }

    // One read covers every row any problem of the table names; each problem
    // then takes the keys of its own rows.
    let mut named: Vec<usize> = Vec::new();
    for problem in &out.items {
        if problem.table.as_deref() == Some(table.name.value.as_str())
            && let Some(rows) = sample_rows(&problem.kind)
        {
            named.extend(rows.iter().copied());
        }
    }
    if named.is_empty() {
        return Ok(());
    }
    let fetched = data_dict_parquet::values_at_rows(
        parquet_path,
        &names,
        &named.iter().map(|row| row - 1).collect::<Vec<_>>(),
    )?;
    let by_row: HashMap<usize, Vec<Option<String>>> = named.into_iter().zip(fetched).collect();

    for problem in &mut out.items {
        if problem.table.as_deref() != Some(table.name.value.as_str()) {
            continue;
        }
        let columns = &problem.columns;
        let Some((rows, keys, redacted)) = sample_mut(&mut problem.kind) else {
            continue;
        };
        let keep: Vec<usize> = (0..names.len())
            .filter(|&j| !columns.contains(&names[j]))
            .collect();
        if rows.is_empty() || keep.is_empty() {
            continue;
        }
        keys.clear();
        keys.extend(rows.iter().map(|row| {
            let values = &by_row[row];
            keep.iter()
                .map(|&j| (names[j].clone(), values[j].clone()))
                .collect()
        }));
        *redacted |= withheld;
    }
    Ok(())
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
    let rowwise: Vec<Vec<String>> = stats
        .orphan_values
        .iter()
        .map(|value| vec![value.clone()])
        .collect();
    let (values, redacted) = value_rows(std::slice::from_ref(&col.name.value), &rowwise, |_| {
        is_restricted(col)
    });
    let detail = found(&values, &stats.orphan_rows, count, redacted);
    let plural = if count == 1 { "" } else { "s" };
    let references = format!("{}.{}", parent_table.name.value, parent_col.name.value);
    Problem {
        code: Some("D05"),
        step: None,
        severity: Severity::Error,
        message: format!("has {count} value{plural} not found in `{references}` ({detail})"),
        table: Some(table.name.value.clone()),
        columns: vec![col.name.value.clone()],
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
            keys: Vec::new(),
            values,
            redacted,
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
        step: None,
        severity: Severity::Warning,
        message: format!(
            "can't be verified against `{references}`: {} values aren't comparable",
            barrier_phrase(reason)
        ),
        table: Some(table.name.value.clone()),
        columns: vec![col.name.value.clone()],
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
