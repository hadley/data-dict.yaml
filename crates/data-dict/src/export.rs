//! Render a data dictionary to the JSON export document (see
//! `site/export.md`).
//!
//! [`export_spec`] resolves the dictionary alone; [`export_data`] additionally
//! validates and profiles each table's source data. Both return the run's
//! [`ProblemSet`] plus the document, which is `None` when the level's
//! validation failed — the same failure the corresponding `validate-*` level
//! reports. At the data level a table whose `source` is missing or unreadable
//! is reported as a warning and exported without its profiles, rather than
//! failing the export.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use data_dict_parquet::{
    ColumnNeeds, ColumnProfile, ColumnRequest, Distinct, ValueKind, edge_scalar, profile,
    profile_paths, render_scalar,
};

use crate::assert_expr::{ColumnsSelector, Expr, ExprKind};
use crate::model::{
    Assertion, Cardinality, Column, Constraint, DataDict, Relationship, Representation, Scalar,
    Table, Version,
};
use crate::problem::{Problem, ProblemKind, ProblemSet, Severity};
use crate::{ReadTables, load, validate_and_lower};

/// The export document. Field order matches the JSON shape documented in
/// `site/export.md`; every "or null" key serializes explicitly (no key is
/// omitted), so consumers see one stable shape.
#[derive(Debug, Serialize)]
pub struct Export {
    name: Option<String>,
    label: Option<String>,
    description: Option<String>,
    details: Option<String>,
    origin: Option<String>,
    learn_more: Option<String>,
    version: Option<ExportVersion>,
    tables: Vec<ExportTable>,
    relationships: Vec<ExportRelationship>,
    glossary: Vec<ExportGlossaryEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum ExportVersion {
    Number(String),
    Date(String),
    Hash(String),
}

#[derive(Debug, Serialize)]
struct ExportTable {
    name: String,
    label: Option<String>,
    description: Option<String>,
    details: Option<String>,
    origin: Option<String>,
    source: Option<ExportSource>,
    columns: Vec<ExportColumn>,
    constraints: Vec<ExportAssertion>,
}

#[derive(Debug, Serialize)]
struct ExportSource {
    parquet: String,
}

#[derive(Debug, Serialize)]
struct ExportColumn {
    name: String,
    label: Option<String>,
    description: Option<String>,
    details: Option<String>,
    display: Option<String>,
    #[serde(rename = "type")]
    col_type: Option<String>,
    units: Option<String>,
    time_zone: Option<String>,
    constraints: Vec<&'static str>,
    references: Option<ExportColumnRef>,
    referenced_by: Vec<ExportColumnRef>,
    values: Option<Vec<JsonScalar>>,
    range: Option<ExportRange>,
    examples: Option<Vec<JsonScalar>>,
    fields: Option<Vec<ExportColumn>>,
    assertions: Vec<ExportAssertion>,
    profile: Option<ExportProfile>,
}

#[derive(Debug, Serialize)]
struct ExportColumnRef {
    table: String,
    column: String,
}

#[derive(Debug, Serialize)]
struct ExportRange {
    min: JsonScalar,
    max: JsonScalar,
}

#[derive(Debug, Serialize)]
struct ExportAssertion {
    expression: String,
    description: Option<String>,
    columns: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ExportRelationship {
    description: Option<String>,
    cardinality: &'static str,
    left: ExportSide,
    right: ExportSide,
    join: String,
    aliases: Vec<ExportAlias>,
    conflicts: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ExportSide {
    table: String,
    columns: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ExportAlias {
    name: String,
    table: String,
}

#[derive(Debug, Serialize)]
struct ExportGlossaryEntry {
    term: String,
    definition: String,
}

#[derive(Debug, Serialize)]
struct ExportProfile {
    distinct: Option<ExportDistinct>,
    missing: Option<usize>,
    sample_values: Vec<JsonScalar>,
    histogram: Option<ExportHistogram>,
    common_values: Option<ExportCommonValues>,
}

#[derive(Debug, Serialize)]
struct ExportDistinct {
    count: usize,
    approximate: bool,
}

#[derive(Debug, Serialize)]
struct ExportHistogram {
    bins: Vec<ExportBin>,
}

#[derive(Debug, Serialize)]
struct ExportBin {
    min: JsonScalar,
    max: JsonScalar,
    count: usize,
    /// Which of the bin's boundary values it includes: `"right"` is `(min,
    /// max]`, `"both"` is `[min, max]` (the first bin, so the column minimum
    /// has a home).
    closed: &'static str,
}

#[derive(Debug, Serialize)]
struct ExportCommonValues {
    approximate: bool,
    values: Vec<ExportValueCount>,
}

#[derive(Debug, Serialize)]
struct ExportValueCount {
    value: JsonScalar,
    count: usize,
}

/// A literal JSON value: the only scalar type in the document.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum JsonScalar {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Null,
}

/// The profiles gathered for one table's readable source, keyed by the
/// column's dotted path (`address.zip` for a struct field).
type TableProfiles = HashMap<String, ExportProfile>;

/// Render the dictionary at `dict_path` without reading any data. The document
/// is `None` when spec validation fails, with the failure in the returned
/// problems — exactly `validate-spec`'s verdict.
pub fn export_spec(dict_path: &Path) -> (ProblemSet, Option<Export>) {
    let (mut problems, doc) = match load(dict_path) {
        Ok(loaded) => loaded,
        Err(problems) => return (problems, None),
    };
    let Some(dict) = validate_and_lower(&doc, &mut problems) else {
        return (problems, None);
    };
    let export = build(&dict, HashMap::new());
    (problems, Some(export))
}

/// Render the dictionary and profile each table's source data. Runs the full
/// metadata- and data-level checks first and returns no document when they
/// error — except a missing (M04) or unreadable (M05) `source`, which is
/// reported as a warning and leaves that table's `profile` fields null, so a
/// partially-sourced dictionary still exports everything it can.
pub fn export_data(dict_path: &Path) -> (ProblemSet, Option<Export>) {
    let (mut problems, doc) = match load(dict_path) {
        Ok(loaded) => loaded,
        Err(problems) => return (problems, None),
    };
    let Some(dict) = validate_and_lower(&doc, &mut problems) else {
        return (problems, None);
    };

    let base_dir = dict_path.parent().unwrap_or_else(|| Path::new(""));
    let mut readable: ReadTables = HashMap::new();
    for table in &dict.tables {
        let Some((parquet_path, actual)) =
            crate::read_parquet(table, base_dir, Severity::Warning, &mut problems)
        else {
            continue;
        };
        crate::validate_meta::meta_issues(table, &actual, &mut problems);
        if let Err(e) =
            crate::validate_data::value_issues(table, &parquet_path, &actual, &mut problems)
        {
            problems.push(Problem::preflight(ProblemKind::Parquet, e.to_string()));
        }
        let columns = actual.iter().map(|col| col.name.clone()).collect();
        readable.insert(table.name.value.clone(), (parquet_path, columns));
    }
    crate::validate_data::foreign_key_issues(&dict, &readable, &mut problems);
    if problems.status().failed() {
        return (problems, None);
    }

    // Profile only once the data is known to match the dictionary, so every
    // declared column resolves in its table's file.
    let mut profiles: HashMap<String, TableProfiles> = HashMap::new();
    for table in &dict.tables {
        let Some((parquet_path, _)) = readable.get(&table.name.value) else {
            continue;
        };
        match profile_table(table, parquet_path) {
            Ok(table_profiles) => {
                profiles.insert(table.name.value.clone(), table_profiles);
            }
            Err(e) => {
                problems.push_located(
                    ProblemKind::UnreadableSource,
                    Severity::Warning,
                    "A table's `source` must point at a readable Parquet file.",
                    e.to_string(),
                    [table.name.span.clone()],
                );
            }
        }
    }
    let export = build(&dict, profiles);
    (problems, Some(export))
}

// --- document assembly -------------------------------------------------

fn build(dict: &DataDict, mut profiles: HashMap<String, TableProfiles>) -> Export {
    Export {
        name: dict.name.clone(),
        label: dict.label.clone(),
        description: dict.description.clone(),
        details: dict.details.clone(),
        origin: dict.origin.clone(),
        learn_more: dict.learn_more.clone(),
        version: dict.version.as_ref().map(|v| match v {
            Version::Number(s) => ExportVersion::Number(s.clone()),
            Version::Date(s) => ExportVersion::Date(s.clone()),
            Version::Hash(s) => ExportVersion::Hash(s.clone()),
        }),
        tables: dict
            .tables
            .iter()
            .map(|table| {
                let mut table_profiles = profiles.remove(&table.name.value).unwrap_or_default();
                build_table(dict, table, &mut table_profiles)
            })
            .collect(),
        relationships: dict
            .relationships
            .iter()
            .filter_map(build_relationship)
            .collect(),
        glossary: dict
            .glossary
            .iter()
            .map(|entry| ExportGlossaryEntry {
                term: entry.term.clone(),
                definition: entry.definition.clone(),
            })
            .collect(),
    }
}

fn build_table(dict: &DataDict, table: &Table, profiles: &mut TableProfiles) -> ExportTable {
    ExportTable {
        name: table.name.value.clone(),
        label: table.label.as_ref().map(|s| s.value.clone()),
        description: table.description.as_ref().map(|s| s.value.clone()),
        details: table.details.as_ref().map(|s| s.value.clone()),
        origin: table.origin.clone(),
        source: table.source.as_ref().map(|s| ExportSource {
            parquet: s.parquet.value.clone(),
        }),
        columns: table
            .columns
            .iter()
            .map(|col| build_column(dict, table, col, &[], profiles))
            .collect(),
        constraints: table
            .constraints
            .iter()
            .map(|a| build_assertion(a, table))
            .collect(),
    }
}

fn build_column(
    dict: &DataDict,
    table: &Table,
    col: &Column,
    prefix: &[&str],
    profiles: &mut TableProfiles,
) -> ExportColumn {
    let path: Vec<&str> = prefix
        .iter()
        .copied()
        .chain([col.name.value.as_str()])
        .collect();

    // Declared constraints plus the ones they imply (`primary_key` implies
    // both `unique` and `required`), in one canonical order.
    let mut constraints = Vec::new();
    if col.has(Constraint::PrimaryKey) {
        constraints.push("primary_key");
    }
    if col.has(Constraint::ForeignKey) {
        constraints.push("foreign_key");
    }
    if col.is_unique_implied() {
        constraints.push("unique");
    }
    if col.is_required_implied() {
        constraints.push("required");
    }

    let references = dict
        .resolve_foreign_key(table, col)
        .map(|(other_table, other_col)| ExportColumnRef {
            table: other_table.name.value.clone(),
            column: other_col.name.value.clone(),
        });
    let referenced_by = if col.has(Constraint::PrimaryKey) {
        referencing_columns(dict, table, col)
    } else {
        Vec::new()
    };

    ExportColumn {
        name: col.name.value.clone(),
        label: col.label.clone(),
        description: col.description.clone(),
        details: col.details.clone(),
        display: col.display.clone(),
        col_type: col.col_type.as_ref().map(|t| t.value.clone()),
        units: col.units.as_ref().map(|u| u.value.clone()),
        time_zone: col.time_zone.as_ref().map(|tz| tz.value.clone()),
        constraints,
        references,
        referenced_by,
        values: col.values.as_ref().map(representation_scalars),
        range: col.range.as_ref().and_then(|range| {
            let [min, max] = range.items.as_slice() else {
                return None;
            };
            Some(ExportRange {
                min: scalar_json(&min.value),
                max: scalar_json(&max.value),
            })
        }),
        examples: col.examples.as_ref().map(representation_scalars),
        fields: col.fields.as_ref().map(|fields| {
            fields
                .iter()
                .map(|field| build_column(dict, table, field, &path, profiles))
                .collect()
        }),
        assertions: col
            .assertions
            .iter()
            .map(|a| build_assertion(a, table))
            .collect(),
        profile: profiles.remove(&path.join(".")),
    }
}

/// Every `foreign_key` column, anywhere in the dictionary, whose relationship
/// resolves to primary-key column `col` of `table`.
fn referencing_columns(dict: &DataDict, table: &Table, col: &Column) -> Vec<ExportColumnRef> {
    let mut out = Vec::new();
    for other_table in &dict.tables {
        for other_col in &other_table.columns {
            let Some((pk_table, pk_col)) = dict.resolve_foreign_key(other_table, other_col) else {
                continue;
            };
            if pk_table.name.value == table.name.value && pk_col.name.value == col.name.value {
                out.push(ExportColumnRef {
                    table: other_table.name.value.clone(),
                    column: other_col.name.value.clone(),
                });
            }
        }
    }
    out
}

fn representation_scalars(rep: &Representation) -> Vec<JsonScalar> {
    rep.items
        .iter()
        .map(|item| scalar_json(&item.value))
        .collect()
}

fn build_assertion(assertion: &Assertion, table: &Table) -> ExportAssertion {
    let mut columns = Vec::new();
    if let Some(expr) = &assertion.expr {
        collect_columns(&expr.root, table, &mut columns);
    }
    ExportAssertion {
        expression: assertion.text.value.clone(),
        description: assertion.description.clone(),
        columns,
    }
}

/// Collect every column (and struct field, dotted) `e` references into `out`,
/// first-appearance order, deduplicated. A `COLUMNS(...)` selection expands to
/// the table columns it matches, mirroring the S21/S22 checker.
fn collect_columns(e: &Expr, table: &Table, out: &mut Vec<String>) {
    match &e.kind {
        ExprKind::Column(path) => push_unique(out, path.join(".")),
        ExprKind::Columns(selector) => match selector {
            ColumnsSelector::All => {
                for col in &table.columns {
                    push_unique(out, col.name.value.clone());
                }
            }
            ColumnsSelector::Regex { pattern, .. } => {
                if let Ok(re) = regex::Regex::new(pattern) {
                    for col in &table.columns {
                        if re.is_match(&col.name.value) {
                            push_unique(out, col.name.value.clone());
                        }
                    }
                }
            }
            ColumnsSelector::List(names) => {
                for named in names {
                    push_unique(out, named.name.clone());
                }
            }
        },
        ExprKind::Neg(inner) | ExprKind::Not(inner) => collect_columns(inner, table, out),
        ExprKind::Arith { lhs, rhs, .. }
        | ExprKind::Compare { lhs, rhs, .. }
        | ExprKind::And(lhs, rhs)
        | ExprKind::Or(lhs, rhs) => {
            collect_columns(lhs, table, out);
            collect_columns(rhs, table, out);
        }
        ExprKind::IsNull { operand, .. } => collect_columns(operand, table, out),
        ExprKind::Between {
            operand, lo, hi, ..
        } => {
            collect_columns(operand, table, out);
            collect_columns(lo, table, out);
            collect_columns(hi, table, out);
        }
        ExprKind::In { operand, list, .. } => {
            collect_columns(operand, table, out);
            for item in list {
                collect_columns(item, table, out);
            }
        }
        ExprKind::Like {
            operand, pattern, ..
        }
        | ExprKind::SimilarTo {
            operand, pattern, ..
        } => {
            collect_columns(operand, table, out);
            collect_columns(pattern, table, out);
        }
        ExprKind::Call { args, .. } => {
            for arg in args {
                collect_columns(arg, table, out);
            }
        }
        ExprKind::Interval { n, .. } => collect_columns(n, table, out),
        ExprKind::Case { whens, els } => {
            for (when, then) in whens {
                collect_columns(when, table, out);
                collect_columns(then, table, out);
            }
            if let Some(els) = els {
                collect_columns(els, table, out);
            }
        }
        ExprKind::Number { .. }
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::Now => {}
    }
}

fn push_unique(out: &mut Vec<String>, name: String) {
    if !out.contains(&name) {
        out.push(name);
    }
}

/// Normalize a relationship so its cardinality reads left-to-right as
/// "many-to-one": a declared `one-to-many` has its sides swapped. `None` only
/// for a join that never parsed, which spec validation rejects before export.
fn build_relationship(rel: &Relationship) -> Option<ExportRelationship> {
    let join = rel.join.as_ref()?;
    let first = join.conjuncts.first()?;
    let side = |name: &str| {
        let mut columns = Vec::new();
        for conjunct in &join.conjuncts {
            for qcol in [&conjunct.lhs, &conjunct.rhs] {
                if qcol.table == name {
                    push_unique(&mut columns, qcol.column.clone());
                }
            }
        }
        ExportSide {
            table: rel.resolve(name).to_string(),
            columns,
        }
    };
    let lhs = side(&first.lhs.table);
    let rhs = side(&first.rhs.table);
    let (cardinality, left, right) = match rel.cardinality.value {
        Cardinality::OneToOne => ("one-to-one", lhs, rhs),
        Cardinality::ManyToOne => ("many-to-one", lhs, rhs),
        Cardinality::OneToMany => ("many-to-one", rhs, lhs),
    };
    Some(ExportRelationship {
        description: rel.description.clone(),
        cardinality,
        left,
        right,
        join: rel.join_text.value.clone(),
        aliases: rel
            .aliases
            .iter()
            .map(|alias| ExportAlias {
                name: alias.name.value.clone(),
                table: alias.table.value.clone(),
            })
            .collect(),
        conflicts: rel.conflicts.iter().map(|c| c.value.clone()).collect(),
    })
}

/// A dictionary scalar as its JSON value. An infinite numeric bound has no
/// JSON spelling and renders as `null`, leaving that end of a range open.
fn scalar_json(scalar: &Scalar) -> JsonScalar {
    match scalar {
        Scalar::Int(n) => JsonScalar::Int(*n),
        Scalar::Float(f) if f.is_finite() => JsonScalar::Float(*f),
        Scalar::Float(_) => JsonScalar::Null,
        Scalar::String(s) => JsonScalar::String(s.clone()),
        Scalar::Bool(b) => JsonScalar::Bool(*b),
        Scalar::Null | Scalar::Compound => JsonScalar::Null,
    }
}

// --- data profiles ------------------------------------------------------

/// Profile every declared column of `table`'s parquet file, keyed by dotted
/// path. Scalar top-level columns get the full single-pass profile; fields of
/// `struct` (and `list(struct)`) columns are profiled per value through
/// [`profile_paths`]; a list-typed column is profiled as the list column
/// itself — its missing count (null containers) — never its elements; a
/// `struct` column carries no profile of its own.
fn profile_table(
    table: &Table,
    parquet_path: &Path,
) -> Result<TableProfiles, data_dict_parquet::ParquetError> {
    let mut scalars: Vec<&str> = Vec::new();
    let mut containers: Vec<String> = Vec::new();
    let mut nested: Vec<Vec<String>> = Vec::new();
    for col in &table.columns {
        let name = std::slice::from_ref(&col.name.value);
        match column_shape(col) {
            Shape::Struct => plan_fields(col, name, &mut nested),
            Shape::List => {
                containers.push(col.name.value.clone());
                plan_fields(col, name, &mut nested);
            }
            Shape::Scalar => scalars.push(&col.name.value),
        }
    }

    let mut out = TableProfiles::new();
    if !scalars.is_empty() {
        let profiled = profile(parquet_path, Some(&scalars))?;
        for column in profiled.columns {
            let name = column.name.clone();
            out.insert(name, profile_json(&column));
        }
    }
    if !nested.is_empty() {
        for (path, profiled) in nested.iter().zip(profile_paths(parquet_path, &nested)?) {
            if let Some(column) = profiled {
                out.insert(path.join("."), profile_json(&column));
            }
        }
    }
    if !containers.is_empty() {
        let requests: Vec<ColumnRequest> = containers
            .iter()
            .map(|name| ColumnRequest {
                path: vec![name.clone()],
                needs: ColumnNeeds {
                    nulls: true,
                    allowed: None,
                },
            })
            .collect();
        let stats = data_dict_parquet::column_stats(parquet_path, &requests, 0)?;
        for (name, stat) in containers.into_iter().zip(stats) {
            out.insert(
                name,
                ExportProfile {
                    distinct: None,
                    missing: Some(stat.null_count),
                    sample_values: Vec::new(),
                    histogram: None,
                    common_values: None,
                },
            );
        }
    }
    Ok(out)
}

/// How a declared column is profiled, from its `type`. An untyped column makes
/// no claims and is profiled as whatever its data turns out to be.
enum Shape {
    Scalar,
    Struct,
    List,
}

fn column_shape(col: &Column) -> Shape {
    let Some(col_type) = &col.col_type else {
        return Shape::Scalar;
    };
    if col_type.value == "struct" {
        Shape::Struct
    } else if col_type.value.starts_with("list(") {
        Shape::List
    } else {
        Shape::Scalar
    }
}

/// Add the paths of every scalar field under `col` to `paths`, recursing
/// through nested structs. A list-typed field carries no profile (its
/// container nulls aren't countable below the top level), but a `list(struct)`
/// field's own fields still profile per element.
fn plan_fields(col: &Column, prefix: &[String], paths: &mut Vec<Vec<String>>) {
    let Some(fields) = &col.fields else { return };
    for field in fields {
        let path: Vec<String> = prefix
            .iter()
            .cloned()
            .chain([field.name.value.clone()])
            .collect();
        match column_shape(field) {
            Shape::Scalar => paths.push(path),
            Shape::Struct | Shape::List => plan_fields(field, &path, paths),
        }
    }
}

/// Shape one engine profile into the export form. `histogram` comes populated
/// for kinds on a numeric scale, `common_values` for text and boolean ones;
/// the other stays null.
fn profile_json(column: &ColumnProfile) -> ExportProfile {
    let kind = &column.kind;
    let distinct = column.distinct.map(|distinct| match distinct {
        Distinct::Exact(count) => ExportDistinct {
            count,
            approximate: false,
        },
        Distinct::Approx(count) => ExportDistinct {
            count,
            approximate: true,
        },
    });
    let histogram = column.histogram.as_ref().and_then(|histogram| {
        let width = histogram
            .bins
            .first()
            .map(|bin| bin.upper - bin.lower)
            .unwrap_or(1.0);
        let bins: Vec<ExportBin> = histogram
            .bins
            .iter()
            .map(|bin| ExportBin {
                min: rendered_json(edge_scalar(bin.lower, kind, width)),
                max: rendered_json(edge_scalar(bin.upper, kind, width)),
                count: bin.count,
                closed: if bin.lower_inclusive { "both" } else { "right" },
            })
            .collect();
        (!bins.is_empty()).then_some(ExportHistogram { bins })
    });
    let common_values =
        matches!(kind, ValueKind::Text | ValueKind::Bool).then(|| ExportCommonValues {
            approximate: column.value_counts.iter().any(|vc| vc.error > 0),
            values: column
                .value_counts
                .iter()
                .map(|vc| ExportValueCount {
                    value: rendered_json(render_scalar(&vc.value, kind)),
                    count: vc.count,
                })
                .collect(),
        });
    ExportProfile {
        distinct,
        missing: column.null_count,
        sample_values: column
            .examples
            .iter()
            .map(|value| rendered_json(render_scalar(value, kind)))
            .collect(),
        histogram,
        common_values,
    }
}

fn rendered_json(scalar: data_dict_parquet::Scalar) -> JsonScalar {
    match scalar {
        data_dict_parquet::Scalar::Bool(b) => JsonScalar::Bool(b),
        data_dict_parquet::Scalar::Int(n) => JsonScalar::Int(n),
        data_dict_parquet::Scalar::Float(f) => JsonScalar::Float(f),
        data_dict_parquet::Scalar::Text(s) => JsonScalar::String(s),
    }
}
