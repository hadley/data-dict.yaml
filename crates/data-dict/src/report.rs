//! The machine-readable report a validation run produces, as specified by
//! `site/report.md`: what the run checked ([`Step`]s) and what it found
//! ([`Problem`](crate::Problem)s).
//!
//! A step is registered for every check the dictionary implies, before the
//! checks run, so a step nobody reached stays [`StepOutcome::Unevaluated`] —
//! the honest verdict for the many paths that abandon a check silently (a
//! column absent from the data, an assertion that reads one). Checks then
//! record a pass or a failure against the step's [`StepKey`].
//!
//! [`Report`] is a borrowing view over a [`ProblemSet`]: it holds the
//! [`SourceContext`] each span needs to become a line/column
//! [`SpanLocation`], which the problems can't carry themselves.

use std::collections::HashMap;

use quarto_source_map::SourceContext;

use crate::Level;
use crate::model::{Column, Constraint, Table};
use crate::problem::{Problem, ProblemSet, SpanLocation, Status, Suggestion, span_location};

/// The version of the report document format itself, not of any dictionary.
pub const REPORT_VERSION: &str = "0.1.0";

/// A step's verdict. `Unevaluated` is the initial state: a step that reached no
/// verdict has not passed, and a consumer must not count it as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepOutcome {
    Pass,
    Fail,
    Unevaluated,
}

/// One check applied to one target: a column, a set of key columns, an
/// assertion, or a table. Steps carry counts but never values, so nothing on
/// one is ever withheld for a `display: restricted` column.
#[derive(Debug, serde::Serialize)]
pub struct Step {
    pub id: usize,
    /// The code this step reports when its target is plainly wrong; a problem
    /// pointing at it may carry an alternative verdict's code (`M02` under an
    /// `M01` step, `D03` under a `D02` one).
    pub code: &'static str,
    pub table: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assertion: Option<String>,
    pub outcome: StepOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_row_count: Option<usize>,
    /// Whether a failure of this step blames every row of the table, which is
    /// only known once the table's rows have been counted.
    #[serde(skip)]
    all_rows_fail: bool,
    /// Whether this step's failure is that the table's rows could never be
    /// counted, so it reports no counts at all.
    #[serde(skip)]
    uncounted: bool,
}

/// What a step checks, within its table. The variants keep targets that share a
/// code apart: a column declared both `unique` and `primary_key` is two `D02`
/// steps, and two identically-worded assertions are two `D07` steps.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum StepTarget {
    /// `M04` — the table itself.
    Table,
    /// `M01` — a declared column, by path segments.
    Column(Vec<String>),
    /// `D01` — a `required` or `primary_key` column.
    Required(String),
    /// `D02` — a `unique` column.
    Unique(String),
    /// `D02` — the primary key as a whole.
    PrimaryKey,
    /// `D04` — an `enum` column, by path segments.
    Enum(Vec<String>),
    /// `D05` — a single-column `foreign_key`.
    ForeignKey(String),
    /// `D07` — an `assert` expression, by its position in [`table_assertions`].
    Assertion(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct StepKey {
    pub table: String,
    pub target: StepTarget,
}

impl StepKey {
    pub(crate) fn new(table: &str, target: StepTarget) -> Self {
        StepKey {
            table: table.to_string(),
            target,
        }
    }
}

/// How many rows a failing step blames.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Failed {
    /// The rows the check counted.
    Rows(usize),
    /// Every row of the table: a single verdict about the whole table, weighted
    /// so it counts alongside the row-level steps.
    AllRows,
    /// None that can be counted, because the table's rows never were (`M04`,
    /// `M05`).
    Uncounted,
}

/// The steps of one run, in the order the dictionary declares them.
#[derive(Debug, Default)]
pub struct Steps {
    steps: Vec<Step>,
    index: HashMap<StepKey, usize>,
    row_counts: HashMap<String, usize>,
}

impl Steps {
    pub fn items(&self) -> &[Step] {
        &self.steps
    }

    fn position(&self, key: &StepKey) -> Option<usize> {
        self.index.get(key).copied()
    }

    pub(crate) fn id(&self, key: &StepKey) -> Option<usize> {
        self.position(key).map(|at| self.steps[at].id)
    }

    /// Register every step the dictionary implies for `table`, all
    /// `Unevaluated` until a check records otherwise. A metadata-level run
    /// registers only the checks it runs. `assertions` are the table's
    /// expressions in [`table_assertions`] order, each with the columns it
    /// reads.
    pub(crate) fn register(
        &mut self,
        table: &Table,
        level: Level,
        assertions: &[(String, Vec<String>)],
    ) {
        let name = table.name.value.as_str();
        let mut pending = vec![(
            StepKey::new(name, StepTarget::Table),
            "M04",
            Vec::new(),
            None,
        )];
        for col in &table.columns {
            column_steps(
                name,
                col,
                level,
                &mut vec![col.name.value.clone()],
                &mut pending,
            );
        }
        if level == Level::Data {
            let key_columns: Vec<String> = table
                .columns
                .iter()
                .filter(|col| col.has(Constraint::PrimaryKey))
                .map(|col| col.name.value.clone())
                .collect();
            if !key_columns.is_empty() {
                pending.push((
                    StepKey::new(name, StepTarget::PrimaryKey),
                    "D02",
                    key_columns,
                    None,
                ));
            }
            for (position, (text, columns)) in assertions.iter().enumerate() {
                pending.push((
                    StepKey::new(name, StepTarget::Assertion(position)),
                    "D07",
                    columns.clone(),
                    Some(text.clone()),
                ));
            }
        }

        // Dictionary order: the table's own step, then each step under the
        // column it is about, then that column's checks by code — the metadata
        // ones before the data ones, as the levels run. A step over several
        // columns sorts by the first of them; one about no column (an aggregate
        // assertion reading none) sorts last.
        let order = column_order(table);
        pending.sort_by_key(|(key, code, columns, _)| {
            let rank = match key.target {
                StepTarget::Table => 0,
                _ => columns
                    .first()
                    .and_then(|column| order.get(column))
                    .map_or(usize::MAX, |at| at + 1),
            };
            (rank, code.starts_with('D'), *code)
        });

        for (key, code, columns, assertion) in pending {
            self.add(key, code, columns, assertion);
        }
    }

    fn add(
        &mut self,
        key: StepKey,
        code: &'static str,
        columns: Vec<String>,
        assertion: Option<String>,
    ) {
        if self.index.contains_key(&key) {
            return;
        }
        self.index.insert(key.clone(), self.steps.len());
        self.steps.push(Step {
            id: self.steps.len() + 1,
            code,
            table: key.table,
            columns,
            assertion,
            outcome: StepOutcome::Unevaluated,
            row_count: None,
            failed_row_count: None,
            all_rows_fail: false,
            uncounted: false,
        });
    }

    pub(crate) fn pass(&mut self, key: &StepKey) {
        if let Some(at) = self.position(key) {
            self.steps[at].outcome = StepOutcome::Pass;
            self.steps[at].failed_row_count = Some(0);
        }
    }

    pub(crate) fn fail(&mut self, key: &StepKey, failed: Failed) {
        if let Some(at) = self.position(key) {
            let step = &mut self.steps[at];
            step.outcome = StepOutcome::Fail;
            match failed {
                Failed::Rows(rows) => step.failed_row_count = Some(rows),
                Failed::AllRows => step.all_rows_fail = true,
                Failed::Uncounted => step.uncounted = true,
            }
        }
    }

    /// Record how many rows the steps of `table` are weighed against.
    pub(crate) fn set_row_count(&mut self, table: &str, rows: usize) {
        self.row_counts.insert(table.to_string(), rows);
    }

    /// Weigh every step against its table's rows, once every check has run. A
    /// step that reached no verdict is left without counts, and so is the
    /// `M04` step of a table whose rows could never be counted.
    pub(crate) fn finish(&mut self) {
        for step in &mut self.steps {
            if step.outcome == StepOutcome::Unevaluated || step.uncounted {
                continue;
            }
            let Some(&rows) = self.row_counts.get(&step.table) else {
                continue;
            };
            step.row_count = Some(rows);
            if step.all_rows_fail {
                step.failed_row_count = Some(rows);
            }
        }
    }
}

/// A step yet to be registered: its key, the code it reports, the columns it
/// covers, and an assertion's text.
type Pending = (StepKey, &'static str, Vec<String>, Option<String>);

/// The steps of one column and, recursively, of its fields — a field is a
/// declared column too, named by its dotted path. Only a top-level column
/// carries constraints, so only it gets the constraint-driven steps.
fn column_steps(
    table: &str,
    col: &Column,
    level: Level,
    path: &mut Vec<String>,
    out: &mut Vec<Pending>,
) {
    let dotted = path.join(".");
    out.push((
        StepKey::new(table, StepTarget::Column(path.clone())),
        "M01",
        vec![dotted.clone()],
        None,
    ));
    if level == Level::Data {
        if path.len() == 1 {
            if col.is_required_implied() {
                out.push((
                    StepKey::new(table, StepTarget::Required(dotted.clone())),
                    "D01",
                    vec![dotted.clone()],
                    None,
                ));
            }
            if col.has(Constraint::Unique) {
                out.push((
                    StepKey::new(table, StepTarget::Unique(dotted.clone())),
                    "D02",
                    vec![dotted.clone()],
                    None,
                ));
            }
            if col.has(Constraint::ForeignKey) {
                out.push((
                    StepKey::new(table, StepTarget::ForeignKey(dotted.clone())),
                    "D05",
                    vec![dotted.clone()],
                    None,
                ));
            }
        }
        if col.is_enum() && col.values.is_some() {
            out.push((
                StepKey::new(table, StepTarget::Enum(path.clone())),
                "D04",
                vec![dotted],
                None,
            ));
        }
    }
    for field in col.fields.iter().flatten() {
        path.push(field.name.value.clone());
        column_steps(table, field, level, path, out);
        path.pop();
    }
}

/// Put the columns a check names into dictionary order, which is the order a
/// step and a problem report them in. An expression names them in the order it
/// reads them, which is its own business.
pub(crate) fn in_dictionary_order(table: &Table, columns: &mut [String]) {
    let order = column_order(table);
    columns.sort_by_key(|column| order.get(column).copied().unwrap_or(usize::MAX));
}

/// Where each column sits in the table, fields depth-first under their parent,
/// keyed by the dotted path a step names it with.
fn column_order(table: &Table) -> HashMap<String, usize> {
    fn walk(columns: &[Column], path: &mut Vec<String>, out: &mut HashMap<String, usize>) {
        for col in columns {
            path.push(col.name.value.clone());
            let next = out.len();
            out.entry(path.join(".")).or_insert(next);
            walk(col.fields.as_deref().unwrap_or(&[]), path, out);
            path.pop();
        }
    }
    let mut out = HashMap::new();
    walk(&table.columns, &mut Vec::new(), &mut out);
    out
}

/// The `assert` expressions of a table, in the order the checks evaluate them:
/// the table's own constraints, then each column's. The position in this
/// sequence identifies the assertion's step, so two identically-worded
/// assertions stay apart.
pub(crate) fn table_assertions(table: &Table) -> Vec<(&crate::model::Assertion, Option<&Column>)> {
    table
        .constraints
        .iter()
        .map(|a| (a, None))
        .chain(
            table
                .columns
                .iter()
                .flat_map(|col| col.assertions.iter().map(move |a| (a, Some(col)))),
        )
        .collect()
}

/// One validation run's findings, ready to serialize as `site/report.md`
/// specifies. Borrows the [`ProblemSet`] for the [`SourceContext`] its spans
/// resolve through.
pub struct Report<'a> {
    problems: &'a ProblemSet,
}

impl<'a> Report<'a> {
    pub fn new(problems: &'a ProblemSet) -> Self {
        Report { problems }
    }
}

#[derive(serde::Serialize)]
struct ReportOut<'a> {
    #[serde(rename = "$version")]
    version: &'static str,
    status: Status,
    steps: &'a [Step],
    problems: Vec<ProblemOut<'a>>,
}

/// A problem plus the parts of it that only resolve against a
/// [`SourceContext`]: its own location, the nodes enclosing it, and a
/// suggestion's insertion point.
#[derive(serde::Serialize)]
struct ProblemOut<'a> {
    #[serde(flatten)]
    problem: &'a Problem,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestion: Option<SuggestionOut<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<SpanLocation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context: Vec<SpanLocation>,
}

#[derive(serde::Serialize)]
struct SuggestionOut<'a> {
    title: &'a str,
    replacement: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<SpanLocation>,
}

impl<'a> SuggestionOut<'a> {
    fn new(suggestion: &'a Suggestion, ctx: &SourceContext) -> Self {
        SuggestionOut {
            title: &suggestion.title,
            replacement: &suggestion.replacement,
            location: span_location(&suggestion.span, ctx),
        }
    }
}

impl serde::Serialize for Report<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let ctx = &self.problems.source;
        let problems = self
            .problems
            .items
            .iter()
            .map(|problem| ProblemOut {
                problem,
                suggestion: problem
                    .suggestion
                    .as_ref()
                    .map(|s| SuggestionOut::new(s, ctx)),
                location: problem.location(ctx),
                context: problem
                    .context_spans()
                    .iter()
                    .filter_map(|span| span_location(span, ctx))
                    .collect(),
            })
            .collect();
        ReportOut {
            version: REPORT_VERSION,
            status: self.problems.status(),
            steps: self.problems.steps.items(),
            problems,
        }
        .serialize(serializer)
    }
}
