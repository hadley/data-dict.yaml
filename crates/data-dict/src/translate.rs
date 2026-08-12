//! Translating a dictionary's assertions into other languages.
//!
//! The unit of translation is one expression, and the output is a **bare
//! predicate** for a caller to embed — not a runnable script. See
//! `site/expression-execution.md` for the targets and what each promises.
//!
//! Output is JSON because this is mostly read by other programs; the `columns`
//! list is what makes it composable, since a caller can tell which columns to
//! load without parsing the code it was given.

use std::path::Path;

use crate::assert_expr::{self, AssertExpr, ColumnRef, Root, TypedAssertion};
use crate::emit::{self, DuckDb, R_BASE, R_DATA_TABLE, R_TIDYVERSE, Target};
use crate::model::{DataDict, Table};
use crate::problem::{Problem, ProblemKind, ProblemSet};
use crate::validate_spec::TableEnv;

/// Every target that can be emitted today, in a stable order.
pub(crate) fn registry() -> Vec<Box<dyn Target>> {
    vec![
        Box::new(DuckDb),
        Box::new(R_TIDYVERSE),
        Box::new(R_BASE),
        Box::new(R_DATA_TABLE),
    ]
}

/// What a bare family name means, per the spec. A default that isn't built yet
/// is still named here, so asking for it says what is missing rather than
/// pretending the family doesn't exist.
const FAMILY_DEFAULTS: &[(&str, &str)] = &[
    ("r", "R(base)"),
    ("python", "Python(polars)"),
    ("sql", "SQL(ANSI)"),
];

/// Resolve a `--target` argument to a target, or say why not.
fn resolve(name: &str) -> Result<Box<dyn Target>, String> {
    let wanted = match FAMILY_DEFAULTS
        .iter()
        .find(|(family, _)| family.eq_ignore_ascii_case(name))
    {
        Some((_, default)) => (*default).to_string(),
        None => name.to_string(),
    };
    if let Some(target) = registry()
        .into_iter()
        .find(|t| t.name().eq_ignore_ascii_case(&wanted))
    {
        return Ok(target);
    }
    let available = registry()
        .iter()
        .map(|t| t.name())
        .collect::<Vec<_>>()
        .join(", ");
    if wanted == name {
        Err(format!("unknown target `{name}`; available: {available}"))
    } else {
        // A family whose default target isn't built yet.
        Err(format!(
            "`{name}` means `{wanted}`, which is not available yet; available: {available}"
        ))
    }
}

/// What to translate, and into what.
#[derive(Debug, Default)]
pub struct Options {
    /// Targets by name; empty means every one available.
    pub targets: Vec<String>,
    /// Restrict to one table, and name the scope for `expr`.
    pub table: Option<String>,
    /// Translate this expression instead of the dictionary's assertions.
    pub expr: Option<String>,
}

/// One expression's translations.
#[derive(Debug, serde::Serialize)]
pub struct Translation {
    /// The expression as written.
    pub expr: String,
    pub table: String,
    /// The expression's own type; `boolean` for an assertion.
    #[serde(rename = "type")]
    pub ty: &'static str,
    /// The columns the expression reads, qualified by table.
    pub columns: Vec<ColumnUse>,
    pub translations: Vec<TargetOutput>,
}

#[derive(Debug, serde::Serialize)]
pub struct ColumnUse {
    pub table: String,
    pub column: String,
}

/// One target's answer: either code, or the reason it refused.
#[derive(Debug, serde::Serialize)]
pub struct TargetOutput {
    pub target: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub notes: Vec<&'static str>,
}

/// Translate the dictionary at `path`.
///
/// `Err` carries the problems that stopped it — a dictionary that doesn't
/// validate, an unknown table, or an `--expr` that doesn't check — with the
/// source needed to render them.
pub fn translate(path: &Path, options: &Options) -> Result<Vec<Translation>, ProblemSet> {
    let (mut problems, doc) = crate::validate_spec::load(path)?;
    let Some(dict) = crate::validate_spec::validate_and_lower(&doc, &mut problems) else {
        return Err(problems);
    };
    // Only translate what checks: a malformed expression has no meaning to
    // carry into another language.
    if problems.status().failed() {
        return Err(problems);
    }

    let targets = match options
        .targets
        .iter()
        .map(|name| resolve(name))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(targets) if !targets.is_empty() => targets,
        Ok(_) => registry(),
        Err(message) => {
            problems.push(Problem::preflight(ProblemKind::Spec, message));
            return Err(problems);
        }
    };

    match &options.expr {
        Some(source) => {
            let table = match scope(&dict, options.table.as_deref()) {
                Ok(table) => table,
                Err(message) => {
                    problems.push(Problem::preflight(ProblemKind::Spec, message));
                    return Err(problems);
                }
            };
            match translate_one(source, table, &targets) {
                Ok(translation) => Ok(vec![translation]),
                Err(message) => {
                    problems.push(Problem::preflight(ProblemKind::Spec, message));
                    Err(problems)
                }
            }
        }
        None => Ok(translate_assertions(
            &dict,
            options.table.as_deref(),
            &targets,
        )),
    }
}

/// The table an ad-hoc expression resolves its columns against: the only one
/// when the dictionary has one, and otherwise the one `--table` names.
fn scope<'a>(dict: &'a DataDict, table: Option<&str>) -> Result<&'a Table, String> {
    let found = match table {
        Some(name) => dict.tables.iter().find(|t| t.name.value == name),
        None if dict.tables.len() == 1 => dict.tables.first(),
        None => None,
    };
    match found {
        Some(table) => Ok(table),
        None => Err(match table {
            Some(name) => format!("no table named `{name}` in this dictionary"),
            None => {
                let names = dict
                    .tables
                    .iter()
                    .map(|t| t.name.value.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "this dictionary has {} tables, so `--table` is needed to say which one \
                         an expression's columns belong to: {names}",
                    dict.tables.len()
                )
            }
        }),
    }
}

/// Parse, check, and translate an ad-hoc expression against one table.
fn translate_one(
    source: &str,
    table: &Table,
    targets: &[Box<dyn Target>],
) -> Result<Translation, String> {
    let env = TableEnv::new(table);
    let expr = AssertExpr::parse(source)
        .map_err(|e| format!("expression does not parse: {}", e.message))?;
    // An ad-hoc expression need not be a rule, so it need not be boolean.
    let findings = assert_expr::check_root(&expr, &env, Root::Any);
    if let Some(finding) = findings
        .iter()
        .find(|f| f.severity == assert_expr::FindingSeverity::Error)
    {
        return Err(format!("[{}] {}", finding.code, finding.message));
    }
    let ir = assert_expr::lower(&expr, &env)
        .ok_or("expression could not be resolved against this table")?;
    Ok(render(source, &table.name.value, &ir, targets))
}

fn translate_assertions(
    dict: &DataDict,
    only: Option<&str>,
    targets: &[Box<dyn Target>],
) -> Vec<Translation> {
    let mut out = Vec::new();
    for table in dict
        .tables
        .iter()
        .filter(|t| only.is_none_or(|name| t.name.value == name))
    {
        let env = TableEnv::new(table);
        let assertions = table
            .constraints
            .iter()
            .chain(table.columns.iter().flat_map(|c| c.assertions.iter()));
        for assertion in assertions {
            let Some(expr) = &assertion.expr else {
                continue;
            };
            let Some(ir) = assert_expr::lower(expr, &env) else {
                continue;
            };
            out.push(render(
                &assertion.text.value,
                &table.name.value,
                &ir,
                targets,
            ));
        }
    }
    out
}

fn render(
    source: &str,
    table: &str,
    ir: &TypedAssertion,
    targets: &[Box<dyn Target>],
) -> Translation {
    Translation {
        expr: source.to_string(),
        table: table.to_string(),
        ty: ir.root.ty.name(),
        columns: ir
            .columns()
            .iter()
            .map(|c: &ColumnRef| ColumnUse {
                table: table.to_string(),
                column: c.path.join("."),
            })
            .collect(),
        // A target that refuses says so and the rest still translate.
        translations: targets
            .iter()
            .map(|target| match emit::emit(target.as_ref(), ir) {
                Ok(emitted) => TargetOutput {
                    target: target.name(),
                    code: Some(emitted.code),
                    error: None,
                    notes: emitted.notes,
                },
                Err(unsupported) => TargetOutput {
                    target: target.name(),
                    code: None,
                    error: Some(format!(
                        "{} is not supported: {}",
                        unsupported.what, unsupported.why
                    )),
                    notes: Vec::new(),
                },
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_family_resolves_to_its_default() {
        // The default is named even when it isn't built yet, so the message
        // says what is missing rather than that the family is unknown.
        let Err(err) = resolve("SQL") else {
            panic!("SQL(ANSI) is not built yet")
        };
        assert!(err.contains("SQL(ANSI)"), "{err}");
        assert!(err.contains("not available yet"), "{err}");
        assert!(resolve("sql(duckdb)").is_ok(), "matching ignores case");
    }

    #[test]
    fn an_unknown_target_lists_the_available_ones() {
        let Err(err) = resolve("Klingon") else {
            panic!("no such target")
        };
        assert!(err.contains("unknown target"), "{err}");
        assert!(err.contains("SQL(duckdb)"), "{err}");
    }

    #[test]
    fn every_registered_target_resolves_by_its_own_name() {
        for target in registry() {
            assert!(resolve(target.name()).is_ok(), "{}", target.name());
        }
    }
}
