//! Core library for the `data-dict.yaml` specification.
//!
//! Validation happens at three levels — [`validate_spec`], [`validate_meta`],
//! [`validate_data`] — each a strict superset of the last; `site/validation.md`
//! defines them and their `S##`/`M##`/`D##` checks.
//!
//! Every level reports its findings as a single [`ProblemSet`]: one vector of
//! [`Problem`]s, whatever their origin (I/O, the schema, a spec check, a
//! metadata or data mismatch). A level pushes its problems and stops the run
//! short by returning early; the meta and data levels validate the spec first
//! and compare against a dataset only when it is free of errors. This module
//! holds the shared [`Level`] and the `compare_dataset` driver the meta and
//! data levels build on.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub mod assert_expr;
mod expr_lex;
pub mod join_expr;
pub mod lower;
pub mod model;
pub mod problem;
pub mod validate_data;
pub mod validate_meta;
pub mod validate_spec;

pub use problem::{Problem, ProblemKind, ProblemSet, RenderStyle, Severity, SpanLocation, Status};
pub use quarto_source_map::SourceContext;
pub use validate_data::validate_data;
pub use validate_meta::validate_meta;
pub(crate) use validate_spec::{load, validate_and_lower};
pub use validate_spec::{validate_spec, validate_spec_str};

use model::{DataDict, Table};

pub const SPEC_MD: &str = include_str!("../../../site/spec.md");

/// Which of the three validation levels a check belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Spec,
    Meta,
    Data,
}

/// The readable data behind the selected tables, keyed by table name: the
/// resolved parquet path and the column names present in it. Passed to the
/// cross-table pass so it can find each table's data and skip a table (or column)
/// that couldn't be read.
pub(crate) type ReadTables = HashMap<String, (PathBuf, Vec<String>)>;

/// The shared prologue for `validate_meta` and `validate_data`, so they differ
/// only in the passes they run: `checks` per table, then `cross` once over the
/// tables that were read (for checks that span tables, like foreign keys).
///
/// Validates the spec first and stops if it has errors. Otherwise it validates
/// every table (or just `table`, when named), locating each table's data through
/// its `source` and reading the parquet file `source.parquet` points at, resolved
/// relative to `dict_path`. A table with no `source` (M04) or an unreadable one
/// (M05) is reported and skipped; the remaining tables are still checked.
pub(crate) fn compare_dataset(
    dict_path: &Path,
    table: Option<&str>,
    checks: impl Fn(&Table, &Path, &[(String, String)], &mut ProblemSet),
    cross: impl Fn(&DataDict, &ReadTables, &mut ProblemSet),
) -> ProblemSet {
    let (mut problems, doc) = match load(dict_path) {
        Ok(loaded) => loaded,
        Err(problems) => return problems,
    };
    let Some(dict) = validate_and_lower(&doc, &mut problems) else {
        return problems;
    };
    let Some(tables) = select_tables(&dict, table, &mut problems) else {
        return problems;
    };
    let base_dir = dict_path.parent().unwrap_or_else(|| Path::new(""));
    let mut readable: ReadTables = HashMap::new();
    for table in tables {
        if let Some((parquet_path, actual)) = read_parquet(table, base_dir, &mut problems) {
            checks(table, &parquet_path, &actual, &mut problems);
            let columns = actual.iter().map(|(name, _)| name.clone()).collect();
            readable.insert(table.name.value.clone(), (parquet_path, columns));
        }
    }
    cross(&dict, &readable, &mut problems);
    problems
}

/// Locate and read a table's data from its `source`, returning the resolved
/// parquet path and its column schema. Reports the source problem and returns
/// `None` when the table has no `source` (M04) or its parquet file can't be read
/// (M05), so the caller skips it.
fn read_parquet(
    table: &Table,
    base_dir: &Path,
    out: &mut ProblemSet,
) -> Option<(PathBuf, Vec<(String, String)>)> {
    let Some(source) = &table.source else {
        out.push_located(
            ProblemKind::MissingSource,
            Severity::Error,
            "A table validated against data must declare a `source`.",
            "has no `source`",
            [table.name.span.clone()],
        );
        return None;
    };
    let parquet_path = base_dir.join(&source.parquet.value);
    match data_dict_parquet::column_types(&parquet_path) {
        Ok(actual) => Some((parquet_path, actual)),
        Err(e) => {
            out.push_located(
                ProblemKind::UnreadableSource,
                Severity::Error,
                "A table's `source` must point at a readable Parquet file.",
                e.to_string(),
                [table.name.span.clone(), source.parquet.span.clone()],
            );
            None
        }
    }
}

/// The tables to validate: the one named by `table`, or all of them. Records a
/// `TableNotFound` pre-flight failure and returns `None` when a named table is
/// absent.
fn select_tables<'a>(
    dict: &'a DataDict,
    table: Option<&str>,
    out: &mut ProblemSet,
) -> Option<Vec<&'a Table>> {
    match table {
        Some(name) => match dict.table(name) {
            Some(table) => Some(vec![table]),
            None => {
                let available = dict
                    .tables
                    .iter()
                    .map(|t| t.name.value.clone())
                    .collect::<Vec<_>>();
                out.push(Problem::preflight(
                    ProblemKind::TableNotFound {
                        available: available.clone(),
                    },
                    format!(
                        "table \"{name}\" is not in the data dictionary (available: {})",
                        available.join(", ")
                    ),
                ));
                None
            }
        },
        None => Some(dict.tables.iter().collect()),
    }
}
