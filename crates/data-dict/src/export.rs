//! Render a data dictionary to the JSON export document (see
//! `site/export.md`).
//!
//! [`export_spec`] resolves the dictionary alone; [`export_data`] additionally
//! profiles each table's source data; [`export_auto`] profiles only when at
//! least one source file is present. All validate the spec first and return
//! the run's [`ProblemSet`] plus the document, which is `None` when that
//! fails — the same failure `validate-spec` reports. The data itself is never
//! validated against the dictionary: a table whose `source` is missing or
//! unreadable is reported as a warning and exported without its profiles.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::Serialize;

use data_dict_parquet::{
    ColumnNeeds, ColumnProfile, ColumnRequest, DataColumn, Distinct, ValueKind, edge_scalar,
    profile, profile_paths, render_scalar,
};

use crate::assert_expr::{ColumnsSelector, Expr, ExprKind};
use crate::model::{
    Assertion, Cardinality, Column, Constraint, DataDict, Relationship, Representation, Scalar,
    Spanned, Table, Version,
};
use crate::problem::{ProblemKind, ProblemSet, Severity};
use crate::{load, validate_and_lower};

/// The version of the export document format itself, carried as the
/// document's `$version` so consumers can detect shape changes.
pub const EXPORT_VERSION: &str = "0.1.0";

/// The export document. Field order matches the JSON shape documented in
/// `site/export.md`. A key with nothing to say — a missing optional, an empty
/// collection — is omitted rather than serialized as `null`/`[]`; zeroes and
/// falses are data and always appear.
#[derive(Debug, Serialize)]
pub struct Export {
    #[serde(rename = "$version")]
    format_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    todo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    learn_more: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<ExportVersion>,
    tables: Vec<ExportTable>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    relationships: Vec<ExportRelationship>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    glossary: Vec<ExportGlossaryEntry>,
}

impl Export {
    /// Each table's source file, in declaration order and still relative to
    /// the dictionary's own directory. Tables without a `source` contribute
    /// nothing, and a path repeated across tables is returned once.
    pub fn source_paths(&self) -> Vec<&Path> {
        let mut paths: Vec<&Path> = Vec::new();
        for table in &self.tables {
            if let Some(source) = &table.source {
                let path = Path::new(&source.parquet);
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
        paths
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    todo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<ExportSource>,
    /// The source data's row count; export-data only, and absent when the
    /// table's source couldn't be profiled.
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<usize>,
    columns: Vec<ExportColumn>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    constraints: Vec<ExportAssertion>,
}

#[derive(Debug, Serialize)]
struct ExportSource {
    parquet: String,
}

#[derive(Debug, Serialize)]
struct ExportColumn {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    todo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<String>,
    #[serde(rename = "type")]
    col_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    units: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time_zone: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    constraints: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    references: Option<ExportColumnRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    referenced_by: Vec<ExportColumnRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    values: Vec<JsonScalar>,
    /// What each value means, keyed by the value, for an enum written in the
    /// map form. Absent for the list form, where the values speak for
    /// themselves.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    value_labels: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<ExportRange>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    examples: Vec<JsonScalar>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fields: Vec<ExportColumn>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    assertions: Vec<ExportAssertion>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    columns: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ExportRelationship {
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    todo: Option<String>,
    cardinality: &'static str,
    /// The cardinality as written in the dictionary — the orientation the
    /// `join` text documents, before any left/right normalization.
    declared_cardinality: &'static str,
    pairs: Vec<ExportPair>,
    join: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<ExportAlias>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    conflicts: Vec<String>,
}

/// One join conjunct as a column correspondence, oriented so `left` is the
/// normalized "many" side; tables are real (alias-resolved) names.
#[derive(Debug, Serialize)]
struct ExportPair {
    left: ExportColumnRef,
    right: ExportColumnRef,
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

/// A column's data profile. The shape follows the column's type, so a key
/// that could never apply to it doesn't appear at all: numeric and temporal
/// columns summarize on a scale (observed range, histogram); string, boolean,
/// and enum columns summarize by value (common values); a list column — and a
/// column whose Parquet type can't be summarised — reports only its missing
/// count.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ExportProfile {
    Scaled {
        #[serde(skip_serializing_if = "Option::is_none")]
        distinct: Option<ExportDistinct>,
        #[serde(skip_serializing_if = "Option::is_none")]
        missing: Option<usize>,
        /// The observed extremes.
        #[serde(skip_serializing_if = "Option::is_none")]
        range: Option<ExportRange>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        sample_values: Vec<JsonScalar>,
        #[serde(skip_serializing_if = "Option::is_none")]
        histogram: Option<ExportHistogram>,
    },
    Valued {
        #[serde(skip_serializing_if = "Option::is_none")]
        distinct: Option<ExportDistinct>,
        #[serde(skip_serializing_if = "Option::is_none")]
        missing: Option<usize>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        sample_values: Vec<JsonScalar>,
        #[serde(skip_serializing_if = "Option::is_none")]
        common_values: Option<ExportCommonValues>,
    },
    Minimal {
        missing: usize,
    },
}

impl ExportProfile {
    /// Drop the sample values. For a column that declares its `values` they
    /// say nothing: the declaration is exhaustive, so the reader already has
    /// every value the column can hold, in full rather than as a sample.
    fn drop_samples(&mut self) {
        match self {
            ExportProfile::Scaled { sample_values, .. }
            | ExportProfile::Valued { sample_values, .. } => sample_values.clear(),
            ExportProfile::Minimal { .. } => {}
        }
    }
}

#[derive(Debug, Serialize)]
struct ExportDistinct {
    count: usize,
    approximate: bool,
}

#[derive(Debug, Serialize)]
struct ExportHistogram {
    bins: Vec<ExportBin>,
    /// Float values with no place on the number line, counted apart from the
    /// bins (as `describe` reports them); each appears only when nonzero.
    #[serde(skip_serializing_if = "is_zero")]
    nan_count: usize,
    #[serde(skip_serializing_if = "is_zero")]
    negative_infinity_count: usize,
    #[serde(skip_serializing_if = "is_zero")]
    positive_infinity_count: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
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
#[derive(Debug, Serialize, PartialEq)]
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

/// Everything gathered from one table's readable source: its row count and
/// the per-column profiles.
struct TableData {
    rows: usize,
    profiles: TableProfiles,
}

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

/// Render the dictionary and profile each table's source data. Validates the
/// spec only — the data itself is never validated against the dictionary
/// (that's `validate-meta`/`validate-data`): a source that's missing (M04) or
/// unreadable (M05) is reported as a warning and that table's profiles are
/// omitted, and a declared column the data doesn't have simply gets no
/// `profile`, so a partially-sourced dictionary still exports everything it
/// can.
pub fn export_data(dict_path: &Path) -> (ProblemSet, Option<Export>) {
    let (mut problems, doc) = match load(dict_path) {
        Ok(loaded) => loaded,
        Err(problems) => return (problems, None),
    };
    let Some(dict) = validate_and_lower(&doc, &mut problems) else {
        return (problems, None);
    };
    let base_dir = dict_path.parent().unwrap_or_else(|| Path::new(""));
    let profiles = profile_sources(&dict, base_dir, &mut problems);
    let export = build(&dict, profiles);
    (problems, Some(export))
}

/// Render the dictionary, profiling source data only when at least one table's
/// source file is actually present: with none, this is [`export_spec`] — no
/// data is read and no missing-source warnings are raised — and otherwise it
/// is [`export_data`], where the tables whose sources are missing still warn.
pub fn export_auto(dict_path: &Path) -> (ProblemSet, Option<Export>) {
    let (mut problems, doc) = match load(dict_path) {
        Ok(loaded) => loaded,
        Err(problems) => return (problems, None),
    };
    let Some(dict) = validate_and_lower(&doc, &mut problems) else {
        return (problems, None);
    };
    let base_dir = dict_path.parent().unwrap_or_else(|| Path::new(""));
    let profiles = if any_source_present(&dict, base_dir) {
        profile_sources(&dict, base_dir, &mut problems)
    } else {
        HashMap::new()
    };
    let export = build(&dict, profiles);
    (problems, Some(export))
}

/// Whether any table's `source.parquet` resolves to an existing file.
fn any_source_present(dict: &DataDict, base_dir: &Path) -> bool {
    dict.tables.iter().any(|table| {
        table
            .source
            .as_ref()
            .is_some_and(|source| base_dir.join(&source.parquet.value).is_file())
    })
}

/// Profile every table whose source can be read, keyed by table name. A source
/// that's missing (M04) or unreadable (M05) is reported as a warning and that
/// table is skipped.
fn profile_sources(
    dict: &DataDict,
    base_dir: &Path,
    problems: &mut ProblemSet,
) -> HashMap<String, TableData> {
    let mut profiles: HashMap<String, TableData> = HashMap::new();
    for table in &dict.tables {
        let Some((parquet_path, actual)) =
            crate::read_parquet(table, base_dir, Severity::Warning, problems)
        else {
            continue;
        };
        match profile_table(table, &parquet_path, &actual) {
            Ok(table_data) => {
                profiles.insert(table.name.value.clone(), table_data);
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
    profiles
}

// --- document assembly -------------------------------------------------

/// Render a Markdown prose field (`description`, `details`, a glossary
/// definition) to HTML, so consumers don't need a Markdown implementation of
/// their own. Raw HTML in the source is escaped rather than passed through,
/// so dictionary text can't smuggle markup into a page that embeds the
/// export.
fn markdown_html(text: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, html};
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let parser = Parser::new_ext(text, options).map(|event| match event {
        Event::Html(s) => Event::Text(s),
        Event::InlineHtml(s) => Event::Text(s),
        other => other,
    });
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out.trim_end().to_string()
}

fn prose(text: &Option<String>) -> Option<String> {
    text.as_deref().map(markdown_html)
}

/// The same, for a field the model keeps with its span.
fn spanned_prose(text: &Option<Spanned<String>>) -> Option<String> {
    text.as_ref().map(|s| markdown_html(&s.value))
}

fn build(dict: &DataDict, mut profiles: HashMap<String, TableData>) -> Export {
    Export {
        format_version: EXPORT_VERSION,
        name: dict.name.clone(),
        label: dict.label.clone(),
        description: prose(&dict.description),
        details: prose(&dict.details),
        todo: spanned_prose(&dict.todo),
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
                let (rows, mut table_profiles) = match profiles.remove(&table.name.value) {
                    Some(data) => (Some(data.rows), data.profiles),
                    None => (None, TableProfiles::new()),
                };
                build_table(dict, table, rows, &mut table_profiles)
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
                definition: markdown_html(&entry.definition),
            })
            .collect(),
    }
}

fn build_table(
    dict: &DataDict,
    table: &Table,
    rows: Option<usize>,
    profiles: &mut TableProfiles,
) -> ExportTable {
    ExportTable {
        name: table.name.value.clone(),
        label: table.label.as_ref().map(|s| s.value.clone()),
        description: spanned_prose(&table.description),
        details: spanned_prose(&table.details),
        todo: spanned_prose(&table.todo),
        origin: table.origin.clone(),
        source: table.source.as_ref().map(|s| ExportSource {
            parquet: s.parquet.value.clone(),
        }),
        rows,
        columns: build_columns(dict, table, &table.columns, &[], profiles),
        constraints: table
            .constraints
            .iter()
            .map(|a| build_assertion(a, table))
            .collect(),
    }
}

/// Build one level of the column tree. A column (or field) with no declared
/// `type` makes no claims and is omitted from the export.
fn build_columns(
    dict: &DataDict,
    table: &Table,
    columns: &[Column],
    prefix: &[&str],
    profiles: &mut TableProfiles,
) -> Vec<ExportColumn> {
    columns
        .iter()
        .filter(|col| col.col_type.is_some())
        .map(|col| build_column(dict, table, col, prefix, profiles))
        .collect()
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

    let values = col
        .values
        .as_ref()
        .map(representation_scalars)
        .unwrap_or_default();
    let value_labels = col
        .values
        .as_ref()
        .map(representation_labels)
        .unwrap_or_default();
    let mut profile = profiles.remove(&path.join("."));
    if let Some(profile) = &mut profile
        && !values.is_empty()
    {
        profile.drop_samples();
    }

    ExportColumn {
        name: col.name.value.clone(),
        label: col.label.clone(),
        description: prose(&col.description),
        details: prose(&col.details),
        todo: spanned_prose(&col.todo),
        display: col.display.clone(),
        col_type: col
            .col_type
            .as_ref()
            .map(|t| t.value.clone())
            .expect("untyped columns are filtered before building"),
        units: col.units.as_ref().map(|u| u.value.clone()),
        time_zone: col.time_zone.as_ref().map(|tz| tz.value.clone()),
        constraints,
        references,
        referenced_by,
        values,
        value_labels,
        range: col.range.as_ref().and_then(|range| {
            let [min, max] = range.items.as_slice() else {
                return None;
            };
            Some(ExportRange {
                min: scalar_json(&min.value),
                max: scalar_json(&max.value),
            })
        }),
        examples: col
            .examples
            .as_ref()
            .map(representation_scalars)
            .unwrap_or_default(),
        fields: col
            .fields
            .as_ref()
            .map(|fields| build_columns(dict, table, fields, &path, profiles))
            .unwrap_or_default(),
        assertions: col
            .assertions
            .iter()
            .map(|a| build_assertion(a, table))
            .collect(),
        profile,
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

/// The label each enum value was written with, keyed by the value. Empty
/// unless the values were given as a map; enum values are strings, so a value
/// is always a usable key.
fn representation_labels(rep: &Representation) -> BTreeMap<String, String> {
    rep.items
        .iter()
        .zip(&rep.labels)
        .filter_map(|(item, label)| match &item.value {
            Scalar::String(value) => Some((value.clone(), label.clone())),
            _ => None,
        })
        .collect()
}

fn build_assertion(assertion: &Assertion, table: &Table) -> ExportAssertion {
    let mut columns = Vec::new();
    if let Some(expr) = &assertion.expr {
        collect_columns(&expr.root, table, &mut columns);
    }
    ExportAssertion {
        expression: assertion.text.value.clone(),
        description: prose(&assertion.description),
        columns,
    }
}

/// Collect every column (and struct field, dotted) `e` references into `out`,
/// first-appearance order, deduplicated. A `COLUMNS(...)` selection expands to
/// the table columns it matches, mirroring the S21/S22 checker — restricted to
/// typed columns, since untyped ones are omitted from the export.
fn collect_columns(e: &Expr, table: &Table, out: &mut Vec<String>) {
    let typed_columns = || table.columns.iter().filter(|col| col.col_type.is_some());
    match &e.kind {
        ExprKind::Column(path) => push_unique(out, path.join(".")),
        ExprKind::Columns(selector) => match selector {
            ColumnsSelector::All => {
                for col in typed_columns() {
                    push_unique(out, col.name.value.clone());
                }
            }
            ColumnsSelector::Regex { pattern, .. } => {
                if let Ok(re) = regex::Regex::new(pattern) {
                    for col in typed_columns() {
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
    let (declared_cardinality, swap) = match rel.cardinality.value {
        Cardinality::OneToOne => ("one-to-one", false),
        Cardinality::ManyToOne => ("many-to-one", false),
        Cardinality::OneToMany => ("one-to-many", true),
    };
    let cardinality = if swap {
        "many-to-one"
    } else {
        declared_cardinality
    };
    // The side names as written in the join (aliases included), oriented so
    // `left_name` is the normalized left side.
    let (left_name, right_name) = if swap {
        (&first.rhs.table, &first.lhs.table)
    } else {
        (&first.lhs.table, &first.rhs.table)
    };

    // One pair per conjunct, whichever way round the conjunct was written; a
    // conjunct that doesn't span both sides pairs nothing.
    let pairs = join
        .conjuncts
        .iter()
        .filter_map(|conjunct| {
            let column_on = |name: &str| {
                [&conjunct.lhs, &conjunct.rhs]
                    .into_iter()
                    .find(|qcol| qcol.table == *name)
                    .map(|qcol| ExportColumnRef {
                        table: rel.resolve(name).to_string(),
                        column: qcol.column.clone(),
                    })
            };
            Some(ExportPair {
                left: column_on(left_name)?,
                right: column_on(right_name)?,
            })
        })
        .collect();
    Some(ExportRelationship {
        description: prose(&rel.description),
        todo: spanned_prose(&rel.todo),
        cardinality,
        declared_cardinality,
        pairs,
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

/// Profile the declared columns of `table`'s parquet file, keyed by dotted
/// path, along with the file's row count. Scalar top-level columns get the
/// full single-pass profile; fields of `struct` (and `list(struct)`) columns
/// are profiled per value through [`profile_paths`]; a list-typed column is
/// profiled as the list column itself — its missing count (null containers) —
/// never its elements; a `struct` column carries no profile of its own.
/// The data is not validated against the dictionary here: a declared column
/// the data doesn't have (`actual` is what it does have) is skipped, as are
/// untyped columns, which the export omits.
fn profile_table(
    table: &Table,
    parquet_path: &Path,
    actual: &[DataColumn],
) -> Result<TableData, data_dict_parquet::ParquetError> {
    let mut scalars: Vec<&str> = Vec::new();
    let mut containers: Vec<String> = Vec::new();
    let mut nested: Vec<Vec<String>> = Vec::new();
    for col in &table.columns {
        if !actual.iter().any(|data| data.name == col.name.value) {
            continue;
        }
        let name = std::slice::from_ref(&col.name.value);
        match column_shape(col) {
            None => {}
            Some(Shape::Struct) => plan_fields(col, name, &mut nested),
            Some(Shape::List) => {
                containers.push(col.name.value.clone());
                plan_fields(col, name, &mut nested);
            }
            Some(Shape::Scalar) => scalars.push(&col.name.value),
        }
    }

    let mut out = TableProfiles::new();
    if !scalars.is_empty() {
        let profiled = profile(parquet_path, Some(&scalars))?;
        for column in profiled.columns {
            if let Some(profile) = profile_json(&column) {
                out.insert(column.name.clone(), profile);
            }
        }
    }
    if !nested.is_empty() {
        for (path, profiled) in nested.iter().zip(profile_paths(parquet_path, &nested)?) {
            if let Some(profile) = profiled.as_ref().and_then(profile_json) {
                out.insert(path.join("."), profile);
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
                ExportProfile::Minimal {
                    missing: stat.null_count,
                },
            );
        }
    }
    Ok(TableData {
        rows: data_dict_parquet::row_count(parquet_path)?,
        profiles: out,
    })
}

/// How a declared column is profiled, from its `type`; `None` for an untyped
/// column, which is omitted from the export.
enum Shape {
    Scalar,
    Struct,
    List,
}

fn column_shape(col: &Column) -> Option<Shape> {
    let col_type = col.col_type.as_ref()?;
    Some(if col_type.value == "struct" {
        Shape::Struct
    } else if col_type.value.starts_with("list(") {
        Shape::List
    } else {
        Shape::Scalar
    })
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
            None => {}
            Some(Shape::Scalar) => paths.push(path),
            Some(Shape::Struct | Shape::List) => plan_fields(field, &path, paths),
        }
    }
}

/// Shape one engine profile into the export form its kind calls for: scaled
/// (numeric and temporal), valued (text and boolean), or — for a kind the
/// engine can't summarize — the minimal missing count, when even that is
/// known.
fn profile_json(column: &ColumnProfile) -> Option<ExportProfile> {
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
    let sample_values: Vec<JsonScalar> = column
        .examples
        .iter()
        .map(|value| rendered_json(render_scalar(value, kind)))
        .collect();

    if kind.is_binnable() {
        let range = match (&column.min, &column.max) {
            (Some(min), Some(max)) => Some(ExportRange {
                min: rendered_json(render_scalar(min, kind)),
                max: rendered_json(render_scalar(max, kind)),
            }),
            _ => None,
        };
        let histogram = column.histogram.as_ref().map(|histogram| {
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
            ExportHistogram {
                bins,
                nan_count: histogram.not_finite.nan_count,
                negative_infinity_count: histogram.not_finite.negative_infinity_count,
                positive_infinity_count: histogram.not_finite.positive_infinity_count,
            }
        });
        Some(ExportProfile::Scaled {
            distinct,
            missing: column.null_count,
            range,
            sample_values,
            histogram,
        })
    } else if matches!(kind, ValueKind::Text | ValueKind::Bool) {
        let common_values = (!column.value_counts.is_empty()).then(|| ExportCommonValues {
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
        Some(ExportProfile::Valued {
            distinct,
            missing: column.null_count,
            sample_values,
            common_values,
        })
    } else {
        column
            .null_count
            .map(|missing| ExportProfile::Minimal { missing })
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
