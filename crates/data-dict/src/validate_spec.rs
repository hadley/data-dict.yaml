//! Spec-level validation, the `S##` checks (see `site/validation.md`).
//!
//! [`validate_spec`] runs two internal passes — a split not surfaced in the CLI:
//!
//! 1. **schema**: structural validation against the embedded `schema.yaml` via
//!    the `quarto-yaml-validation` crate — everything a JSON Schema can express.
//! 2. **spec**: the cross-table semantic checks below that the schema can't
//!    express (foreign-key targets, `join` parsing, cardinality, …).
//!
//! The second pass only runs if the first succeeds: there is no point chasing
//! FK references in a document whose `tables` block is malformed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};
use quarto_source_map::SourceInfo;
use quarto_yaml::YamlWithSourceInfo;
use quarto_yaml_validation::error::ValidationErrorKind;
use quarto_yaml_validation::{Schema, SchemaRegistry, ValidationDiagnostic, ValidationError};

use crate::assert_expr::{self, CheckEnv, ColumnKind};
use crate::join_expr::{JoinExpr, QCol};
use crate::model::{
    Assertion, Cardinality, Column, DataDict, Representation, Scalar, Spanned, Table,
};
use crate::problem::{Problem, ProblemKind, ProblemSet, Suggestion, subspan};
use crate::{SourceContext, lower};

/// The canonical documentation URL suggested for `$learn_more`.
pub const LEARN_MORE_URL: &str = "http://data-dict.tidyverse.org/";

/// The spec version this validator implements, suggested for a missing `$version`.
pub const SPEC_VERSION: &str = "0.1.0";

const SCHEMA_YAML: &str = include_str!("../../../schema.yaml");

fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let yaml =
            quarto_yaml::parse(SCHEMA_YAML).expect("embedded schema.yaml must be parseable YAML");
        Schema::from_yaml(&yaml).expect("embedded schema.yaml must compile to a valid schema")
    })
}

/// Validate the `data-dict.yaml` file at `path`. The returned [`ProblemSet`]
/// bundles every problem (errors and warnings, in source order) with the source
/// context needed to render them; [`ProblemSet::status`] reports whether the
/// document is valid. Failures that prevent checking altogether — I/O,
/// unparseable YAML, a structurally invalid document — surface as pre-flight
/// [`Problem`]s in the set.
pub fn validate_spec(path: &Path) -> ProblemSet {
    let (mut problems, doc) = match load(path) {
        Ok(loaded) => loaded,
        Err(problems) => return problems,
    };
    // We only want the problems here, not the lowered dictionary.
    validate_and_lower(&doc, &mut problems);
    problems
}

/// The content twin of [`validate_spec`]: validates in-memory `content` (no I/O,
/// for an unsaved buffer), attributing problems to `filename`.
pub fn validate_spec_str(content: &str, filename: &str) -> ProblemSet {
    let (mut problems, doc) = match load_str(content, filename) {
        Ok(loaded) => loaded,
        Err(problems) => return problems,
    };
    validate_and_lower(&doc, &mut problems);
    problems
}

/// Read, parse, and schema-check the document at `path`, creating the run's
/// [`ProblemSet`] with the document's source — this is where every level starts.
/// `Ok((problems, doc))` hands back the fresh set and the parsed AST to validate;
/// `Err(problems)` carries a pre-flight failure (I/O, unparseable YAML, or a
/// document the schema rejects) for which no document could be produced.
pub(crate) fn load(path: &Path) -> Result<(ProblemSet, YamlWithSourceInfo), ProblemSet> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) => return Err(ProblemSet::from_preflight(ProblemKind::Io, e.to_string())),
    };
    let filename = path.display().to_string();
    load_str(&content, &filename)
}

/// The content twin of [`load`]: parses and schema-checks in-memory `content`
/// without reading a file, so it never fails with [`ProblemKind::Io`].
pub(crate) fn load_str(
    content: &str,
    filename: &str,
) -> Result<(ProblemSet, YamlWithSourceInfo), ProblemSet> {
    let doc = match quarto_yaml::parse_file(content, filename) {
        Ok(doc) => doc,
        Err(e) => {
            return Err(ProblemSet::from_preflight(
                ProblemKind::Parse,
                e.to_string(),
            ));
        }
    };

    let mut source = SourceContext::new();
    let file_id = quarto_yaml::file_id_for_filename(filename);
    source.add_file_with_id(file_id, filename.to_string(), Some(content.to_string()));

    let registry = SchemaRegistry::new();
    if let Err(err) = quarto_yaml_validation::validate(&doc, schema(), &registry, &source) {
        // Lift the structural error into our own vocabulary so it renders through
        // the annotate-snippets pipeline like every other diagnostic, rather than
        // the validator's own (ariadne) text.
        let diagnostic = ValidationDiagnostic::from_validation_error(&err, &source);
        let span = schema_error_span(&err);
        let hints = diagnostic.hints();
        let hint = (!hints.is_empty()).then(|| hints.join(" "));
        let mut problems = ProblemSet::new(source);
        problems.push(Problem::schema(
            err.error_code(),
            schema_expected(&err.kind),
            err.message(),
            span,
            hint,
        ));
        return Err(problems);
    }

    Ok((ProblemSet::new(source), doc))
}

/// The tightest span for a structural error. The validator points an unknown
/// property at its enclosing object; narrow that to the offending key so the
/// annotation lands on the property itself. Other errors keep the node the
/// validator attached.
fn schema_error_span(err: &ValidationError) -> Option<SourceInfo> {
    let node = err.yaml_node.as_ref()?;
    if let ValidationErrorKind::UnknownProperty { property } = &err.kind
        && let Some(entry) = node.as_hash().and_then(|entries| {
            entries
                .iter()
                .find(|e| e.key.yaml.as_str() == Some(property))
        })
    {
        return Some(entry.key_span.clone());
    }
    Some(node.source_info.clone())
}

/// The general rule a structural error violates, stated independently of the
/// offending value, to lead the diagnostic (the validator's own `message`
/// carries the concrete finding). Sentence case, ending with a full stop, like
/// every other `expected`.
fn schema_expected(kind: &ValidationErrorKind) -> &'static str {
    use ValidationErrorKind::*;
    match kind {
        TypeMismatch { .. } => "A value must have the type its schema requires.",
        MissingRequiredProperty { .. } => "A required property must be present.",
        UnknownProperty { .. } => "An object may only contain the properties its schema defines.",
        InvalidEnumValue { .. } => "A value must be one of its schema's allowed values.",
        NumberOutOfRange { .. } => "A number must fall within its allowed range.",
        NumberNotMultipleOf { .. } => "A number must be a multiple of its schema's step.",
        StringLengthInvalid { .. } => "A string's length must be within its allowed bounds.",
        StringPatternMismatch { .. } => "A string must match its schema's pattern.",
        ArrayLengthInvalid { .. } => "An array's length must be within its allowed bounds.",
        ArrayItemsNotUnique => "An array's items must be unique.",
        ObjectPropertyCountInvalid { .. } => {
            "An object's property count must be within its allowed bounds."
        }
        UnresolvedReference { .. } => "A schema reference must resolve.",
        DuplicateKey { .. } => "A mapping key must not appear more than once.",
        Other { .. } => "The document must satisfy the schema.",
    }
}

/// Lower the parsed document `doc` and run the S## semantic checks, pushing any
/// findings into `out`. Returns the lowered dictionary when the spec validates,
/// or `None` when it has errors (which `out` then carries).
pub(crate) fn validate_and_lower(
    doc: &YamlWithSourceInfo,
    out: &mut ProblemSet,
) -> Option<DataDict> {
    let dict = lower::lower(doc, out);
    check_spec(&dict, out);
    validate_s09_learn_more(doc, out);
    validate_s17_version(doc, out);
    validate_s18_version_present(doc, out);
    out.sort();

    if out.status().failed() {
        None
    } else {
        Some(dict)
    }
}

/// Run every rule, pushing any findings into `out`; call [`ProblemSet::sort`]
/// afterwards to put the findings in source order.
///
/// Relationship-level rules run against the whole dictionary. Column-level rules
/// run per column from a single iteration here, sequenced so a more specific
/// check runs only when the general one it refines passed: a malformed `name`
/// blocks the uniqueness check, and the representation chain narrows from "the
/// right key is present" (S07) to "its values have the right type" (S12) to "the
/// range is ordered" (S13).
fn check_spec(dict: &DataDict, out: &mut ProblemSet) {
    validate_s02_relationship_table_refs(dict, out);
    validate_s03_relationship_column_refs(dict, out);
    validate_s04_join_table_count(dict, out);
    validate_s05_conflicts_present_on_both_sides(dict, out);
    validate_s06_cardinality_consistency(dict, out);
    validate_s16_single_table_description(dict, out);

    let mut seen_tables: HashMap<String, SourceInfo> = HashMap::new();
    for table in &dict.tables {
        if validate_s11_table_name(table, out) {
            validate_s10_unique_table_name(table, &mut seen_tables, out);
        }
        validate_table_assertions(table, out);
        let mut seen: HashMap<String, SourceInfo> = HashMap::new();
        for col in &table.columns {
            validate_s01_foreign_key(dict, table, col, out);
            validate_s08_units(table, col, out);
            validate_s14_time_zone(table, col, out);
            validate_s15_time_zone_format(table, col, out);
            validate_column_assertions(table, col, out);
            if validate_s11_column_name(table, col, out) {
                validate_s10_unique_name(table, col, &mut seen, out);
            }
            validate_enum_values(table, col, out);
            if validate_s07_representation(table, col, out)
                && validate_s12_value_types(table, col, out)
            {
                validate_s13_range_order(table, col, out);
            }
        }
    }
}

// --- S20 / S21 (assertion semantics) ---------------------------------------

/// A [`CheckEnv`] over one table: it resolves column kinds and parses the date
/// literals an assertion may compare against a `date`/`datetime` column.
struct TableEnv<'a> {
    table: &'a Table,
}

impl TableEnv<'_> {
    fn kind_of(col: &Column) -> ColumnKind {
        match col.col_type.as_ref().map(|t| t.value.as_str()) {
            Some("string") => ColumnKind::String,
            Some("number" | "number(id)" | "number(ordinal)" | "number(quantity)") => {
                ColumnKind::Number
            }
            Some("boolean") => ColumnKind::Bool,
            Some("date") => ColumnKind::Date,
            Some("datetime") => ColumnKind::Datetime,
            // An enum's values are its categories, and those are always strings.
            Some("enum") => ColumnKind::String,
            _ => ColumnKind::Untyped,
        }
    }
}

impl CheckEnv for TableEnv<'_> {
    fn column(&self, name: &str) -> Option<ColumnKind> {
        self.table.column(name).map(Self::kind_of)
    }

    fn columns(&self) -> Vec<(String, ColumnKind)> {
        self.table
            .columns
            .iter()
            .map(|c| (c.name.value.clone(), Self::kind_of(c)))
            .collect()
    }

    fn is_date(&self, s: &str) -> bool {
        parse_date(s).is_some()
    }

    fn is_datetime(&self, s: &str) -> bool {
        // Accept either an offset-bearing or a zoneless ISO 8601 datetime; the
        // column's `time_zone` decides which is canonical, but for a literal
        // comparison either spelling is a legitimate datetime.
        parse_datetime(s).is_some() || parse_naive_datetime(s).is_some()
    }
}

fn validate_column_assertions(table: &Table, col: &Column, out: &mut ProblemSet) {
    let env = TableEnv { table };
    for assertion in &col.assertions {
        run_assertion_check(&env, assertion, &[&table.name, &col.name], out);
    }
}

fn validate_table_assertions(table: &Table, out: &mut ProblemSet) {
    let env = TableEnv { table };
    for assertion in &table.constraints {
        run_assertion_check(&env, assertion, &[&table.name], out);
    }
}

/// Run the S20/S21/S22 checks for one parsed assertion, turning each finding
/// into a located problem. `enclosing` are the outer nodes (table, and column
/// for a column assertion) shown as context before the offending token.
fn run_assertion_check(
    env: &dyn CheckEnv,
    assertion: &Assertion,
    enclosing: &[&Spanned<String>],
    out: &mut ProblemSet,
) {
    use crate::assert_expr::FindingSeverity;

    let Some(expr) = &assertion.expr else { return };
    for finding in assert_expr::check(expr, env) {
        let span = subspan(&assertion.text.span, finding.start, finding.end)
            .unwrap_or_else(|| assertion.text.span.clone());
        let mut spans: Vec<SourceInfo> = enclosing.iter().map(|s| s.span.clone()).collect();
        spans.push(span);
        let expected = match finding.code {
            "S20" => "An assertion may only reference columns of its table.",
            "S22" => "A `COLUMNS(...)` selection should match at least one column.",
            "S23" => "An assertion may only use columns with a declared `type`.",
            _ => "An assertion must be a well-typed boolean expression.",
        };
        match finding.severity {
            FindingSeverity::Error => {
                out.push_spec_error(finding.code, expected, finding.message, spans)
            }
            FindingSeverity::Warning => {
                out.push_spec_warning(finding.code, expected, finding.message, spans)
            }
        }
    }
}

// --- S02 --------------------------------------------------------------

fn validate_s02_relationship_table_refs(dict: &DataDict, out: &mut ProblemSet) {
    for rel in &dict.relationships {
        let Some(join) = &rel.join else { continue };
        for q in join.qcols() {
            if dict.table(&q.table).is_none() {
                let span = subspan(&rel.join_text.span, q.start, q.end)
                    .unwrap_or_else(|| rel.join_text.span.clone());
                out.push_spec_error(
                    "S02",
                    "A `join` must refer to known tables.",
                    format!("table `{}` is not defined", q.table),
                    [span],
                );
            }
        }
    }
}

// --- S03 --------------------------------------------------------------

fn validate_s03_relationship_column_refs(dict: &DataDict, out: &mut ProblemSet) {
    for rel in &dict.relationships {
        if let Some(join) = &rel.join {
            for q in join.qcols() {
                // Skip if the table doesn't exist — S02 handles that case
                // and a column report would be noise.
                let Some(table) = dict.table(&q.table) else {
                    continue;
                };
                if table.column(&q.column).is_none() {
                    let span = subspan(&rel.join_text.span, q.start, q.end)
                        .unwrap_or_else(|| rel.join_text.span.clone());
                    out.push_spec_error(
                        "S03",
                        "A `join` must refer to known columns.",
                        format!("table `{}` has no column `{}`", q.table, q.column),
                        [span],
                    );
                }
            }
        }
        // `conflicts` column references are checked by S05 alongside the
        // "appears on both sides" check, so a missing column there reports
        // the more specific message.
    }
}

// --- S04 --------------------------------------------------------------

fn validate_s04_join_table_count(dict: &DataDict, out: &mut ProblemSet) {
    // Parse failures are emitted during lowering. Here we only check the
    // table-count invariant on successfully parsed joins.
    for rel in &dict.relationships {
        let Some(join) = &rel.join else { continue };
        let tables = join.tables();
        if tables.is_empty() || tables.len() > 2 {
            out.push_spec_error(
                "S04",
                "A `join` must reference one (self-join) or two tables.",
                format!("this `join` references {} tables", tables.len()),
                [rel.join_text.span.clone()],
            );
        }
    }
}

// --- S01 --------------------------------------------------------------

fn validate_s01_foreign_key(dict: &DataDict, table: &Table, col: &Column, out: &mut ProblemSet) {
    use crate::model::Constraint::*;

    if !col.has(ForeignKey) {
        return;
    }
    if dict.resolve_foreign_key(table, col).is_none() {
        let fk_span = col
            .constraints
            .iter()
            .find(|c| c.value == ForeignKey)
            .map_or_else(|| col.name.span.clone(), |c| c.span.clone());
        out.push_spec_error(
            "S01",
            "Every `foreign_key` column must have a matching relationship to a `primary_key`.",
            "is `foreign_key` but no relationship points it at a `primary_key`",
            [table.name.span.clone(), col.name.span.clone(), fk_span],
        );
    }
}

// --- S05 --------------------------------------------------------------

fn validate_s05_conflicts_present_on_both_sides(dict: &DataDict, out: &mut ProblemSet) {
    for rel in &dict.relationships {
        if rel.conflicts.is_empty() {
            continue;
        }
        let Some(join) = &rel.join else { continue };
        let tables = join.tables();
        // For a self-join, the "both sides" reduces to the single table; for
        // a normal join, both tables must contain the column.
        for c in &rel.conflicts {
            let mut missing_from: Vec<&str> = Vec::new();
            for t_name in &tables {
                let Some(table) = dict.table(t_name) else {
                    // S02 already flagged the missing table; skip to avoid
                    // a cascade of confusing reports.
                    continue;
                };
                if table.column(&c.value).is_none() {
                    missing_from.push(*t_name);
                }
            }
            if !missing_from.is_empty() {
                out.push_spec_error(
                    "S05",
                    "A `conflicts` entry must name a column on both sides of the join.",
                    format!(
                        "`{}` is not a column of {}",
                        c.value,
                        join_with_commas(&missing_from)
                    ),
                    [rel.join_text.span.clone(), c.span.clone()],
                );
            }
        }
    }
}

fn join_with_commas(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("`{s}`")).collect();
    match quoted.len() {
        0 => String::new(),
        1 => quoted[0].clone(),
        _ => {
            let (last, init) = quoted.split_last().unwrap();
            format!("{} and {}", init.join(", "), last)
        }
    }
}

// --- S06 --------------------------------------------------------------

fn validate_s06_cardinality_consistency(dict: &DataDict, out: &mut ProblemSet) {
    for rel in &dict.relationships {
        let Some(join) = &rel.join else { continue };

        // Skip if any join column references a missing table or column. The
        // missing reference is already reported (S02 / S03), and checking
        // cardinality against a column that doesn't exist would just produce a
        // redundant, confusing S06.
        let all_cols_resolve = join.qcols().all(|q| {
            dict.table(&q.table)
                .is_some_and(|t| t.column(&q.column).is_some())
        });
        if !all_cols_resolve {
            continue;
        }

        // The cardinality rule is defined in terms of the LHS and RHS tables
        // of the join. With multi-conjunct joins (date-range overlap), the
        // LHS and RHS tables are the same across all conjuncts, so we can
        // use the first conjunct as the canonical orientation.
        let Some(first) = join.conjuncts.first() else {
            continue;
        };
        let lhs_table = first.lhs.table.clone();
        let rhs_table = first.rhs.table.clone();

        // Which columns are "the join side" for each table?  For the
        // single-conjunct equality case this is straightforward. For
        // multi-conjunct (range) joins we require ALL conjunct columns on the
        // "one" side to be jointly unique-implied — in practice users
        // typically mark just one of them as PK/unique. We err on the
        // permissive side and check whether *any* column on the "one" side
        // is unique-implied; that matches the loose intuition behind range
        // joins without producing noise for legitimate overlap joins.

        let lhs_cols_unique =
            side_has_unique_implied(dict, &lhs_table, join, /* use_lhs = */ true);
        let rhs_cols_unique =
            side_has_unique_implied(dict, &rhs_table, join, /* use_lhs = */ false);

        let card_span = rel.cardinality.span.clone();
        match rel.cardinality.value {
            Cardinality::OneToOne => {
                if !lhs_cols_unique || !rhs_cols_unique {
                    out.push_spec_error(
                        "S06",
                        "A `one-to-one` join must have `primary_key` or `unique` columns on both sides.",
                        format!(
                            "the join columns on `{}` or `{}` are not marked `primary_key` or `unique`",
                            lhs_table, rhs_table
                        ),
                        [
                            rel.join_text.span.clone(),
                            card_span,
                        ],
                    );
                }
            }
            Cardinality::OneToMany => {
                // Spec: "from left to right" — one row on the left maps to
                // many on the right, so the left side is the "one" side.
                if !lhs_cols_unique {
                    out.push_spec_error(
                        "S06",
                        "A `one-to-many` join must have a `primary_key` or `unique` column on its left (\"one\") side.",
                        format!(
                            "the left-side join column on `{}` is not marked `primary_key` or `unique`",
                            lhs_table
                        ),
                        [
                            rel.join_text.span.clone(),
                            card_span,
                        ],
                    );
                }
            }
            Cardinality::ManyToOne => {
                if !rhs_cols_unique {
                    out.push_spec_error(
                        "S06",
                        "A `many-to-one` join must have a `primary_key` or `unique` column on its right (\"one\") side.",
                        format!(
                            "the right-side join column on `{}` is not marked `primary_key` or `unique`",
                            rhs_table
                        ),
                        [
                            rel.join_text.span.clone(),
                            card_span,
                        ],
                    );
                }
            }
        }
    }
}

fn side_has_unique_implied(
    dict: &DataDict,
    table_name: &str,
    join: &JoinExpr,
    use_lhs: bool,
) -> bool {
    let Some(table) = dict.table(table_name) else {
        return false;
    };
    join.conjuncts.iter().any(|conj| {
        let q: &QCol = if use_lhs { &conj.lhs } else { &conj.rhs };
        if q.table != table_name {
            return false;
        }
        table
            .column(&q.column)
            .is_some_and(|c| c.is_unique_implied())
    })
}

// --- S07 --------------------------------------------------------------

const RANGE_TYPES: &[&str] = &["number(ordinal)", "number(quantity)", "date", "datetime"];

/// Returns whether the column carries the representation its type requires and
/// no other — i.e. whether checking that representation's values (S12) makes
/// sense.
fn validate_s07_representation(table: &Table, col: &Column, out: &mut ProblemSet) -> bool {
    let Some(col_type) = &col.col_type else {
        return true;
    };
    let type_name = col_type.value.as_str();

    let found = |key: &str| format!("has type `{type_name}` but uses `{key}`");
    let missing = |key: &str| format!("has type `{type_name}` but is missing `{key}`");

    // A finding is reported on a specific line — the `type` line for a missing
    // representation, the offending key's line for a present one — with the
    // table and column shown as faded context above it.
    let at = |span: &SourceInfo| [table.name.span.clone(), col.name.span.clone(), span.clone()];

    let before = out.items.len();
    if type_name == "enum" {
        if col.values.is_none() {
            out.push_spec_error(
                "S07",
                "An `enum` column must list its categories with `values`.",
                missing("values"),
                at(&col_type.span),
            );
        }
        if let Some(range) = &col.range {
            out.push_spec_error(
                "S07",
                "An `enum` column must use `values`, not `range`.",
                found("range"),
                at(&range.span),
            );
        }
        if let Some(examples) = &col.examples {
            out.push_spec_error(
                "S07",
                "An `enum` column must use `values`, not `examples`.",
                found("examples"),
                at(&examples.span),
            );
        }
    } else if RANGE_TYPES.contains(&type_name) {
        if col.range.is_none() {
            out.push_spec_error(
                "S07",
                format!("A `{type_name}` column must describe its bounds with `range`."),
                missing("range"),
                at(&col_type.span),
            );
        }
        if let Some(values) = &col.values {
            out.push_spec_error(
                "S07",
                format!("A `{type_name}` column must use `range`, not `values`."),
                found("values"),
                at(&values.span),
            );
        }
        if let Some(examples) = &col.examples {
            out.push_spec_error(
                "S07",
                format!("A `{type_name}` column must use `range`, not `examples`."),
                found("examples"),
                at(&examples.span),
            );
        }
    } else if type_name == "boolean" {
        for (span, key) in [
            (col.values.as_ref().map(|v| &v.span), "values"),
            (col.range.as_ref().map(|r| &r.span), "range"),
            (col.examples.as_ref().map(|e| &e.span), "examples"),
        ] {
            if let Some(span) = span {
                out.push_spec_error(
                    "S07",
                    "A `boolean` column must not have `values`, `range`, or `examples`.",
                    found(key),
                    at(span),
                );
            }
        }
    } else {
        if col.examples.is_none() {
            out.push_spec_error(
                "S07",
                format!("A `{type_name}` column must describe its data with `examples`."),
                missing("examples"),
                at(&col_type.span),
            );
        }
        if let Some(values) = &col.values {
            out.push_spec_error(
                "S07",
                format!("A `{type_name}` column must not use `values`."),
                found("values"),
                at(&values.span),
            );
        }
        if let Some(range) = &col.range {
            out.push_spec_error(
                "S07",
                format!("A `{type_name}` column must not use `range`."),
                found("range"),
                at(&range.span),
            );
        }
    }
    out.items.len() == before
}

// --- S08 --------------------------------------------------------------

fn validate_s08_units(table: &Table, col: &Column, out: &mut ProblemSet) {
    let Some(units) = &col.units else { return };
    let is_quantity = col
        .col_type
        .as_ref()
        .is_some_and(|t| t.value == "number(quantity)");
    if is_quantity {
        return;
    }
    let type_desc = col
        .col_type
        .as_ref()
        .map_or_else(|| "no type".to_string(), |t| format!("type `{}`", t.value));
    out.push_spec_error(
        "S08",
        "A column with `units` must have type `number(quantity)`.",
        format!("has `units` but has {type_desc}"),
        [
            table.name.span.clone(),
            col.name.span.clone(),
            units.span.clone(),
        ],
    );
}

// --- S14 --------------------------------------------------------------

fn validate_s14_time_zone(table: &Table, col: &Column, out: &mut ProblemSet) {
    let Some(time_zone) = &col.time_zone else {
        return;
    };
    let is_datetime = col.col_type.as_ref().is_some_and(|t| t.value == "datetime");
    if is_datetime {
        return;
    }
    let type_desc = col
        .col_type
        .as_ref()
        .map_or_else(|| "no type".to_string(), |t| format!("type `{}`", t.value));
    let mut spans = vec![table.name.span.clone(), col.name.span.clone()];
    if let Some(col_type) = &col.col_type {
        spans.push(col_type.span.clone());
    }
    spans.push(time_zone.span.clone());
    out.push_spec_error(
        "S14",
        "A column with `time_zone` must have type `datetime`.",
        format!("has `time_zone` but has {type_desc}"),
        spans,
    );
}

// --- S15 --------------------------------------------------------------

/// The IANA areas the `Area/Location` form accepts, mirroring the spec's
/// `Time zones` section: the continents and oceans plus `Etc`.
const TIME_ZONE_AREAS: &[&str] = &[
    "Africa",
    "America",
    "Antarctica",
    "Arctic",
    "Asia",
    "Atlantic",
    "Australia",
    "Europe",
    "Indian",
    "Pacific",
    "Etc",
];

/// Whether `tz` has the accepted `time_zone` shape: `naive`, `UTC`, or an
/// `Area/Location` name with a known area. Checks the shape, not the full tzdb,
/// so the accepted set doesn't go stale as zones are added or renamed.
fn time_zone_well_formed(tz: &str) -> bool {
    if tz == "naive" || tz == "UTC" {
        return true;
    }
    let Some((area, location)) = tz.split_once('/') else {
        return false;
    };
    TIME_ZONE_AREAS.contains(&area)
        && !location.is_empty()
        && location
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-' | '/'))
}

fn validate_s15_time_zone_format(table: &Table, col: &Column, out: &mut ProblemSet) {
    let Some(time_zone) = &col.time_zone else {
        return;
    };
    if time_zone_well_formed(&time_zone.value) {
        return;
    }
    let mut spans = vec![table.name.span.clone(), col.name.span.clone()];
    if let Some(col_type) = &col.col_type {
        spans.push(col_type.span.clone());
    }
    spans.push(time_zone.span.clone());
    out.push_spec_error(
        "S15",
        "A `time_zone` must be `naive`, `UTC`, or an IANA `Area/Location` name.",
        "is not a valid time zone",
        spans,
    );
}

// --- S10 --------------------------------------------------------------

fn validate_s10_unique_name(
    table: &Table,
    col: &Column,
    seen: &mut HashMap<String, SourceInfo>,
    out: &mut ProblemSet,
) {
    match seen.get(&col.name.value) {
        Some(first) => out.push_spec_error(
            "S10",
            "Column names must be unique within a table.",
            "is duplicated",
            [
                table.name.span.clone(),
                first.clone(),
                col.name.span.clone(),
            ],
        ),
        None => {
            seen.insert(col.name.value.clone(), col.name.span.clone());
        }
    }
}

fn validate_s10_unique_table_name(
    table: &Table,
    seen: &mut HashMap<String, SourceInfo>,
    out: &mut ProblemSet,
) {
    match seen.get(&table.name.value) {
        Some(first) => out.push_spec_error(
            "S10",
            "Table names must be unique within the dictionary.",
            "is duplicated",
            [first.clone(), table.name.span.clone()],
        ),
        None => {
            seen.insert(table.name.value.clone(), table.name.span.clone());
        }
    }
}

// --- S11 --------------------------------------------------------------

/// Returns whether the table has a name (so its uniqueness may be checked).
fn validate_s11_table_name(table: &Table, out: &mut ProblemSet) -> bool {
    if table.name.value.is_empty() {
        out.push_spec_error(
            "S11",
            "A table must have a non-empty name.",
            "table name is empty",
            [table.name.span.clone()],
        );
        return false;
    }
    true
}

/// Returns whether the column has a name (so its uniqueness may be checked).
fn validate_s11_column_name(table: &Table, col: &Column, out: &mut ProblemSet) -> bool {
    if col.name.value.is_empty() {
        out.push_spec_error(
            "S11",
            "Every column must have a non-empty `name`.",
            "the `name` is empty",
            [table.name.span.clone(), col.name.span.clone()],
        );
        return false;
    }
    true
}

// --- S12 --------------------------------------------------------------

/// The representation list whose values are type-checked for a given column
/// type, or `None` for types that carry no typed representation (`enum`,
/// `boolean`, and any unrecognized type). Mirrors S07: each type owns exactly
/// one representation key, and we only check the one it owns so that a
/// misplaced key reports as S07 rather than cascading into S12.
fn typed_representation(col: &Column) -> Option<(&'static str, &Representation)> {
    match col.col_type.as_ref()?.value.as_str() {
        "number(ordinal)" | "number(quantity)" | "date" | "datetime" => {
            Some(("range", col.range.as_ref()?))
        }
        "string" | "number" | "number(id)" => Some(("examples", col.examples.as_ref()?)),
        _ => None,
    }
}

/// Returns whether every value in the column's typed representation matches its
/// type — i.e. whether the bounds are sound enough to compare for order (S13).
fn validate_s12_value_types(table: &Table, col: &Column, out: &mut ProblemSet) -> bool {
    let Some(col_type) = col.col_type.as_ref() else {
        return true;
    };
    let type_name = col_type.value.as_str();
    let Some((key, rep)) = typed_representation(col) else {
        return true;
    };
    let tz_present = col.time_zone.is_some();
    let mut ok = true;
    for v in &rep.items {
        if value_matches_type(type_name, &v.value, tz_present) {
            continue;
        }
        ok = false;
        out.push_spec_error(
            "S12",
            format!(
                "Each `{}` value of a `{}` column must be {}.",
                key,
                type_name,
                expected_noun(type_name, tz_present),
            ),
            format!("is {}", v.value.noun()),
            [
                table.name.span.clone(),
                col.name.span.clone(),
                col_type.span.clone(),
                rep.key_span.clone(),
                v.span.clone(),
            ],
        );
        if type_name == "string"
            && let Some(hint) = quoting_hint(&v.value)
        {
            out.hint_last(hint);
        }
    }
    ok
}

fn is_infinite(value: &Scalar) -> bool {
    matches!(value, Scalar::Float(f) if f.is_infinite())
}

fn value_matches_type(type_name: &str, value: &Scalar, tz_present: bool) -> bool {
    match type_name {
        "number" | "number(id)" | "number(ordinal)" | "number(quantity)" => {
            matches!(value, Scalar::Int(_) | Scalar::Float(_))
        }
        "string" => matches!(value, Scalar::String(_)),
        // An infinite bound leaves that end of a temporal range open (spec:
        // Representative values), so accept it alongside a real ISO 8601 value.
        "date" => {
            is_infinite(value) || matches!(value, Scalar::String(s) if parse_date(s).is_some())
        }
        "datetime" => {
            is_infinite(value)
                || matches!(value, Scalar::String(s) if datetime_parses(s, tz_present))
        }
        _ => true,
    }
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    s.parse().ok()
}

fn parse_datetime(s: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(s).ok()
}

fn parse_naive_datetime(s: &str) -> Option<NaiveDateTime> {
    s.parse().ok()
}

/// A `datetime` column carries its zone in `time_zone` (`tz_present`), so its
/// values are written zoneless; without one, each value must carry its own
/// offset.
fn datetime_parses(s: &str, tz_present: bool) -> bool {
    if tz_present {
        parse_naive_datetime(s).is_some()
    } else {
        parse_datetime(s).is_some()
    }
}

fn expected_noun(type_name: &str, tz_present: bool) -> &'static str {
    match type_name {
        "string" => "a string",
        "date" => "an ISO 8601 date (YYYY-MM-DD)",
        "datetime" if tz_present => "a zoneless ISO 8601 datetime (e.g. 2024-01-31T09:30:00)",
        "datetime" => "an ISO 8601 datetime with a timezone (e.g. 2024-01-31T09:30:00Z)",
        _ => "a number",
    }
}

// --- S24 --------------------------------------------------------------

/// An enum's values are the categories of the column, so there must be some and
/// each must be a string. Both forms reach here the same way: the map form's
/// keys are lowered as its values.
fn validate_enum_values(table: &Table, col: &Column, out: &mut ProblemSet) {
    let Some(col_type) = col.col_type.as_ref().filter(|t| t.value == "enum") else {
        return;
    };
    // A missing `values` is S07's to report.
    let Some(values) = &col.values else { return };
    // The `type` and `values` lines come along so the finding reads as being
    // about this column's categories, not a bare scalar somewhere in the file.
    let at = |span: &SourceInfo| {
        [
            table.name.span.clone(),
            col.name.span.clone(),
            col_type.span.clone(),
            values.key_span.clone(),
            span.clone(),
        ]
    };

    if values.items.is_empty() {
        out.push_spec_error(
            "S24",
            "An `enum` column must list at least one value.",
            "is empty",
            at(&values.span),
        );
        return;
    }
    for item in &values.items {
        if !matches!(item.value, Scalar::String(_)) {
            out.push_spec_error(
                "S24",
                "An `enum`'s values must be strings.",
                format!("is {}", item.value.noun()),
                at(&item.span),
            );
            if let Some(hint) = quoting_hint(&item.value) {
                out.hint_last(hint);
            }
        }
    }
}

/// A category or `string` value written unquoted is resolved by its text, so a
/// numeric or boolean spelling arrives as the wrong kind; the fix is to quote
/// it. `None` for a value no quoting rescues (null, or a nested list/map).
fn quoting_hint(value: &Scalar) -> Option<String> {
    let literal = match value {
        Scalar::Int(n) => n.to_string(),
        Scalar::Float(n) => n.to_string(),
        Scalar::Bool(b) => b.to_string(),
        _ => return None,
    };
    Some(format!("Quote it to make it a string: `'{literal}'`."))
}

// --- S13 --------------------------------------------------------------

/// Only reached when S12 confirmed both bounds parse for the column's type, so
/// `range_descending` can compare them meaningfully.
fn validate_s13_range_order(table: &Table, col: &Column, out: &mut ProblemSet) {
    let Some(type_name) = col.col_type.as_ref().map(|t| t.value.as_str()) else {
        return;
    };
    let Some(range) = &col.range else { return };
    if !RANGE_TYPES.contains(&type_name) || range.items.len() != 2 {
        return;
    }
    let (lo, hi) = (&range.items[0], &range.items[1]);
    if range_descending(type_name, &lo.value, &hi.value, col.time_zone.is_some()) {
        out.push_spec_error(
            "S13",
            "A range's minimum must be less than or equal to its maximum.",
            "is greater than the maximum",
            [
                table.name.span.clone(),
                col.name.span.clone(),
                lo.span.clone(),
            ],
        );
    }
}

/// Whether `lo`..`hi` runs backwards for the column's type. Returns `false`
/// unless both bounds parse as the type's value (a mistyped bound is S12's to
/// report). Numbers compare numerically; dates and datetimes compare as parsed
/// instants, so mixed timezone offsets are handled correctly. A datetime column
/// with a `time_zone` (`tz_present`) has zoneless bounds, compared as wall-clock.
/// An infinite bound orders as `-inf` < any value < `+inf`, so it runs backwards
/// only when it sits on the wrong end (`+inf` as minimum, `-inf` as maximum).
fn range_descending(type_name: &str, lo: &Scalar, hi: &Scalar, tz_present: bool) -> bool {
    if is_infinite(lo) || is_infinite(hi) {
        let is_pos = |v: &Scalar| matches!(v, Scalar::Float(f) if *f == f64::INFINITY);
        let is_neg = |v: &Scalar| matches!(v, Scalar::Float(f) if *f == f64::NEG_INFINITY);
        return (is_pos(lo) && !is_pos(hi)) || (is_neg(hi) && !is_neg(lo));
    }
    match (type_name, lo, hi) {
        ("date", Scalar::String(a), Scalar::String(b)) => match (parse_date(a), parse_date(b)) {
            (Some(a), Some(b)) => a > b,
            _ => false,
        },
        ("datetime", Scalar::String(a), Scalar::String(b)) if tz_present => {
            match (parse_naive_datetime(a), parse_naive_datetime(b)) {
                (Some(a), Some(b)) => a > b,
                _ => false,
            }
        }
        ("datetime", Scalar::String(a), Scalar::String(b)) => {
            match (parse_datetime(a), parse_datetime(b)) {
                (Some(a), Some(b)) => a > b,
                _ => false,
            }
        }
        (_, lo, hi) => match (lo.as_f64(), hi.as_f64()) {
            (Some(a), Some(b)) => a > b,
            _ => false,
        },
    }
}

// --- S16 --------------------------------------------------------------

/// Warn when a single-table dictionary carries `label`, `description`, or
/// `details` on the table: for one table, those describe the dataset as a whole
/// and belong at the top level.
fn validate_s16_single_table_description(dict: &DataDict, out: &mut ProblemSet) {
    if dict.tables.len() != 1 {
        return;
    }
    let table = dict.tables.first().expect("one table");
    let present: Vec<(&str, &SourceInfo)> = [
        ("label", &table.label),
        ("description", &table.description),
        ("details", &table.details),
    ]
    .into_iter()
    .filter_map(|(key, opt)| opt.as_ref().map(|span| (key, span)))
    .collect();

    if present.is_empty() {
        return;
    }

    let keys: Vec<&str> = present.iter().map(|(k, _)| *k).collect();
    let keys_fmt = fmt_backtick_list(&keys);
    let verb = if keys.len() == 1 { "belongs" } else { "belong" };
    let mut spans = vec![table.name.span.clone()];
    spans.extend(present.iter().map(|(_, s)| (*s).clone()));

    out.push_spec_warning(
        "S16",
        format!("A single-table dictionary's {keys_fmt} {verb} at the top level."),
        format!("table `{}` has {keys_fmt}", table.name.value),
        spans,
    );
}

fn fmt_backtick_list(keys: &[&str]) -> String {
    match keys {
        [] => String::new(),
        [k] => format!("`{k}`"),
        [a, b] => format!("`{a}` and `{b}`"),
        _ => {
            let init = keys[..keys.len() - 1]
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{init}, and `{}`", keys[keys.len() - 1])
        }
    }
}

// --- S09 --------------------------------------------------------------

/// Warn when the document omits the recommended `$learn_more` key. Unlike the
/// other rules this inspects the raw AST, because `$learn_more` is top-level
/// metadata that the lowered [`DataDict`] does not carry. The key has no
/// location of its own, so the warning is anchored at the document's first
/// character.
fn validate_s09_learn_more(root: &YamlWithSourceInfo, out: &mut ProblemSet) {
    let Some(entries) = root.as_hash() else {
        return;
    };
    let has = |key: &str| entries.iter().find(|e| e.key.yaml.as_str() == Some(key));
    if has("$learn_more").is_some() {
        return;
    }
    let span = subspan(&root.source_info, 0, 1).unwrap_or_else(|| root.source_info.clone());
    // Insert the recommended key at the very start of the document.
    let insert_at = subspan(&root.source_info, 0, 0).unwrap_or_else(|| span.clone());
    out.push_spec_warning(
        "S09",
        "A document should point readers to the spec with `$learn_more`.",
        "`$learn_more` is not set",
        [span],
    );
    out.suggest_last(Suggestion {
        title: "point readers to the spec".into(),
        replacement: format!("$learn_more: {LEARN_MORE_URL}\n"),
        span: insert_at,
    });
}

// --- S17 --------------------------------------------------------------

/// Check the optional top-level `version`. The schema has already fixed its
/// shape (a map whose only keys are `number`, `date`, or `hash`, each with the
/// right value type); S17 enforces the semantic rules the schema can't: a
/// `version` must carry exactly one of those keys, a `number` must have three
/// dot-separated numeric components (with an optional suffix), and a `date` must
/// be a valid ISO 8601 date. Like S09, it reads the raw AST because `version` is
/// top-level metadata the lowered [`DataDict`] does not carry.
fn validate_s17_version(root: &YamlWithSourceInfo, out: &mut ProblemSet) {
    let Some(entries) = root.as_hash() else {
        return;
    };
    let Some(version) = entries
        .iter()
        .find(|e| e.key.yaml.as_str() == Some("version"))
    else {
        return;
    };
    let Some(fields) = version.value.as_hash() else {
        return;
    };

    let expected = "A `version` must give exactly one of `number`, `date`, or `hash`.";
    match fields {
        [] => {
            out.push_spec_error(
                "S17",
                expected,
                "names none of them",
                [version.key_span.clone()],
            );
            return;
        }
        [_] => {}
        [first, .., last] => {
            // The last key is the offending one: a kind was already supplied
            // before it. Highlight it, with the `version` key and the kind it
            // duplicates shown faded above.
            let already = first.key.yaml.as_str().unwrap_or("");
            out.push_spec_error(
                "S17",
                expected,
                format!("`{already}` has already been supplied"),
                [
                    version.key_span.clone(),
                    first.key_span.clone(),
                    last.key_span.clone(),
                ],
            );
            return;
        }
    }

    let field = &fields[0];
    let spans = || [version.key_span.clone(), field.value_span.clone()];
    let text = field.value.yaml.as_str();
    match field.key.yaml.as_str() {
        Some("number") if text.is_none_or(|s| !is_version_number(s)) => {
            out.push_spec_error(
                "S17",
                "A `version` `number` must have three dot-separated numeric components, with an optional pre-release/build suffix.",
                match text {
                    Some(s) => format!("`{s}` is not a valid version number"),
                    None => "is not a valid version number".to_string(),
                },
                spans(),
            );
        }
        Some("date") if text.is_none_or(|s| parse_date(s).is_none()) => {
            out.push_spec_error(
                "S17",
                "A `version` `date` must be an ISO 8601 date (YYYY-MM-DD).",
                match text {
                    Some(s) => format!("`{s}` is not an ISO 8601 date"),
                    None => "is not an ISO 8601 date".to_string(),
                },
                spans(),
            );
        }
        _ => {}
    }
}

// --- S18 --------------------------------------------------------------

/// Error when the document omits the required top-level `$version` key. The
/// schema leaves this key optional so its absence lands here with a patch
/// (a present-but-wrong value is still an enum error at the schema level).
/// Like S09, the missing key has no location of its own, so the error is
/// anchored at the document's first character.
fn validate_s18_version_present(root: &YamlWithSourceInfo, out: &mut ProblemSet) {
    let Some(entries) = root.as_hash() else {
        return;
    };
    if entries
        .iter()
        .any(|e| e.key.yaml.as_str() == Some("$version"))
    {
        return;
    }
    let span = subspan(&root.source_info, 0, 1).unwrap_or_else(|| root.source_info.clone());
    // Insert the key at the very start of the document.
    let insert_at = subspan(&root.source_info, 0, 0).unwrap_or_else(|| span.clone());
    out.push_spec_error(
        "S18",
        "A document must declare the spec version it conforms to with `$version`.",
        "`$version` is not set",
        [span],
    );
    out.suggest_last(Suggestion {
        title: "declare the spec version".into(),
        replacement: format!("$version: {SPEC_VERSION}\n"),
        span: insert_at,
    });
}

/// A version `number` per the spec: three dot-separated numeric components
/// (`MAJOR.MINOR.PATCH`), with an optional semver pre-release (`-…`) and/or
/// build (`+…`) suffix whose dot-separated identifiers are alphanumeric or `-`.
fn is_version_number(s: &str) -> bool {
    let (rest, build) = match s.split_once('+') {
        Some((rest, build)) => (rest, Some(build)),
        None => (s, None),
    };
    let (core, pre) = match rest.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (rest, None),
    };

    let mut parts = core.split('.');
    let numeric = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let core_ok = matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(a), Some(b), Some(c), None) if numeric(a) && numeric(b) && numeric(c)
    );

    let suffix_ok = |s: &str| {
        s.split('.')
            .all(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    };

    core_ok && pre.is_none_or(suffix_ok) && build.is_none_or(suffix_ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_schema_compiles() {
        let _ = schema();
    }
}
