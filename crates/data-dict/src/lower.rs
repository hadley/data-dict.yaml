//! Lower a `quarto_yaml` AST into the typed [`DataDict`] model.
//!
//! Invariant: lowering only runs after the schema has accepted the document,
//! so we may assume the shape conforms (required keys present, enums valid,
//! arrays where arrays are expected). Unexpected shapes are silently dropped
//! rather than panicking — they should be unreachable.

use quarto_source_map::SourceInfo;
use quarto_yaml::YamlWithSourceInfo;

use crate::assert_expr::AssertExpr;
use crate::join_expr::JoinExpr;
use crate::model::{
    Alias, Assertion, Cardinality, Column, Constraint, DataDict, Definition, GlossaryEntry,
    Relationship, Representation, Scalar, Source, Spanned, Table, Version,
};
use crate::problem::{Problem, ProblemSet, Severity, subspan};

/// Lower an AST, collecting any lowering problems (currently only S04
/// for unparseable join expressions).
pub fn lower(root: &YamlWithSourceInfo, problems: &mut ProblemSet) -> DataDict {
    let mut tables = Vec::new();
    if let Some(t_node) = root.get_hash_value("tables")
        && let Some(items) = t_node.as_array()
    {
        for item in items {
            if let Some(table) = lower_table(item, problems) {
                tables.push(table);
            }
        }
    }

    let mut relationships = Vec::new();
    if let Some(r_node) = root.get_hash_value("relationships")
        && let Some(items) = r_node.as_array()
    {
        for item in items {
            relationships.push(lower_relationship(item, problems));
        }
    }

    let todo = root.as_hash().and_then(|entries| {
        entries
            .iter()
            .find(|e| e.key.yaml.as_str() == Some("todo"))
            .and_then(|e| lower_todo(&e.value, e.value_span.clone()))
    });

    let string_value = |key: &str| {
        root.get_hash_value(key)
            .and_then(|n| n.yaml.as_str())
            .map(str::to_string)
    };

    let mut glossary = Vec::new();
    if let Some(g_node) = root.get_hash_value("glossary")
        && let Some(entries) = g_node.as_hash()
    {
        for entry in entries {
            if let (Some(term), Some(definition)) =
                (entry.key.yaml.as_str(), entry.value.yaml.as_str())
            {
                glossary.push(GlossaryEntry {
                    term: term.to_string(),
                    definition: definition.to_string(),
                });
            }
        }
    }

    DataDict {
        name: string_value("name"),
        label: string_value("label"),
        description: string_value("description"),
        details: string_value("details"),
        origin: string_value("origin"),
        learn_more: string_value("$learn_more"),
        version: root.get_hash_value("version").and_then(lower_version),
        tables,
        relationships,
        glossary,
        todo,
    }
}

/// Lower the top-level `version` map. The schema fixes the value types and S17
/// enforces exactly one key; when several slipped through (S17 fails the run)
/// the first is kept so lowering still returns something well-formed. A
/// `number` may be written unquoted (e.g. `1.0`), so non-strings are rendered
/// back to text.
fn lower_version(node: &YamlWithSourceInfo) -> Option<Version> {
    let entry = node.as_hash()?.first()?;
    let yaml = &entry.value.yaml;
    let text = yaml.as_str().map(str::to_string).or_else(|| {
        yaml.as_i64()
            .map(|n| n.to_string())
            .or_else(|| yaml.as_f64().map(|n| n.to_string()))
    })?;
    match entry.key.yaml.as_str()? {
        "number" => Some(Version::Number(text)),
        "date" => Some(Version::Date(text)),
        "hash" => Some(Version::Hash(text)),
        _ => None,
    }
}

/// Lower a `todo` value — a single string — with its span.
fn lower_todo(value: &YamlWithSourceInfo, value_span: SourceInfo) -> Option<Spanned<String>> {
    value
        .yaml
        .as_str()
        .map(|s| Spanned::new(s.to_string(), value_span))
}

fn lower_table(node: &YamlWithSourceInfo, problems: &mut ProblemSet) -> Option<Table> {
    let entries = node.as_hash()?;
    let name_entry = entries
        .iter()
        .find(|e| e.key.yaml.as_str() == Some("name"))?;
    // An empty/null name is kept (as "") so S11 can report it; the parser
    // collapses an empty name to null.
    let name = name_entry.value.yaml.as_str().unwrap_or("");

    let mut columns = Vec::new();
    if let Some(c_node) = node.get_hash_value("columns")
        && let Some(items) = c_node.as_array()
    {
        for col in items {
            if let Some(c) = lower_column(col, problems) {
                columns.push(c);
            }
        }
    }
    let mut constraints = Vec::new();
    if let Some(c_node) = node.get_hash_value("constraints")
        && let Some(items) = c_node.as_array()
    {
        for item in items {
            if let Some(a) = lower_assertion(item, problems) {
                constraints.push(a);
            }
        }
    }
    let mut definitions = Vec::new();
    if let Some(d_node) = node.get_hash_value("definitions")
        && let Some(items) = d_node.as_array()
    {
        for item in items {
            if let Some(d) = lower_definition(item, problems) {
                definitions.push(d);
            }
        }
    }
    let source = node.get_hash_value("source").and_then(|n| {
        let parquet = n.get_hash_value("parquet")?;
        let path = parquet.yaml.as_str()?;
        Some(Source {
            span: n.source_info.clone(),
            parquet: Spanned::new(path.to_string(), parquet.source_info.clone()),
        })
    });
    // The value with the key's own span, so S16 can point at the key line.
    let keyed_string = |key: &str| {
        entries
            .iter()
            .find(|e| e.key.yaml.as_str() == Some(key))
            .map(|e| {
                Spanned::new(
                    e.value.yaml.as_str().unwrap_or("").to_string(),
                    e.key_span.clone(),
                )
            })
    };
    let todo = entries
        .iter()
        .find(|e| e.key.yaml.as_str() == Some("todo"))
        .and_then(|e| lower_todo(&e.value, e.value_span.clone()));
    let origin = node
        .get_hash_value("origin")
        .and_then(|n| n.yaml.as_str())
        .map(str::to_string);
    Some(Table {
        span: node.source_info.clone(),
        name: Spanned::new(name.to_string(), name_entry.value_span.clone()),
        columns,
        constraints,
        definitions,
        source,
        label: keyed_string("label"),
        description: keyed_string("description"),
        details: keyed_string("details"),
        origin,
        todo,
    })
}

fn lower_column(node: &YamlWithSourceInfo, problems: &mut ProblemSet) -> Option<Column> {
    let entries = node.as_hash()?;
    let mut name: Option<Spanned<String>> = None;
    let mut label: Option<String> = None;
    let mut description: Option<String> = None;
    let mut details: Option<String> = None;
    let mut display: Option<String> = None;
    let mut constraints: Vec<Spanned<Constraint>> = Vec::new();
    let mut assertions: Vec<Assertion> = Vec::new();
    let mut col_type: Option<Spanned<String>> = None;
    let mut values: Option<Representation> = None;
    let mut range: Option<Representation> = None;
    let mut examples: Option<Representation> = None;
    let mut units: Option<Spanned<String>> = None;
    let mut time_zone: Option<Spanned<String>> = None;
    let mut fields: Option<Vec<Column>> = None;
    let mut todo: Option<Spanned<String>> = None;
    for entry in entries {
        let Some(key) = entry.key.yaml.as_str() else {
            continue;
        };
        match key {
            "name" => {
                // An empty/null name is kept (as "") so S11 can report it; the
                // parser collapses an empty name to null.
                let s = entry.value.yaml.as_str().unwrap_or("");
                name = Some(Spanned::new(s.to_string(), entry.value_span.clone()));
            }
            "type" => {
                if let Some(s) = entry.value.yaml.as_str() {
                    col_type = Some(Spanned::new(s.to_string(), entry.value_span.clone()));
                }
            }
            "label" => label = entry.value.yaml.as_str().map(str::to_string),
            "description" => description = entry.value.yaml.as_str().map(str::to_string),
            "details" => details = entry.value.yaml.as_str().map(str::to_string),
            "display" => display = entry.value.yaml.as_str().map(str::to_string),
            "values" => {
                let (items, labels) = lower_enum_values(&entry.value);
                values = Some(Representation {
                    span: entry.value_span.clone(),
                    key_span: entry.key_span.clone(),
                    items,
                    labels,
                });
            }
            "range" => {
                range = Some(Representation {
                    span: entry.value_span.clone(),
                    key_span: entry.key_span.clone(),
                    items: lower_scalars(&entry.value),
                    labels: Vec::new(),
                });
            }
            "examples" => {
                examples = Some(Representation {
                    span: entry.value_span.clone(),
                    key_span: entry.key_span.clone(),
                    items: lower_scalars(&entry.value),
                    labels: Vec::new(),
                });
            }
            "units" => {
                if let Some(s) = entry.value.yaml.as_str() {
                    units = Some(Spanned::new(s.to_string(), entry.value_span.clone()));
                }
            }
            "time_zone" => {
                if let Some(s) = entry.value.yaml.as_str() {
                    time_zone = Some(Spanned::new(s.to_string(), entry.value_span.clone()));
                }
            }
            "constraints" => {
                if let Some(items) = entry.value.as_array() {
                    for c in items {
                        if let Some(s) = c.yaml.as_str() {
                            // A bareword names a structural constraint.
                            if let Some(parsed) = Constraint::parse(s) {
                                constraints.push(Spanned::new(parsed, c.source_info.clone()));
                            }
                        } else if let Some(a) = lower_assertion(c, problems) {
                            // A map with an `assert` key is an assertion.
                            assertions.push(a);
                        }
                    }
                }
            }
            "fields" => {
                if let Some(items) = entry.value.as_array() {
                    let mut fs = Vec::new();
                    for f in items {
                        if let Some(col) = lower_column(f, problems) {
                            fs.push(col);
                        }
                    }
                    fields = Some(fs);
                }
            }
            "todo" => {
                todo = lower_todo(&entry.value, entry.value_span.clone());
            }
            _ => {}
        }
    }
    Some(Column {
        name: name?,
        label,
        description,
        details,
        display,
        constraints,
        assertions,
        col_type,
        values,
        range,
        examples,
        units,
        time_zone,
        fields,
        todo,
    })
}

/// Lower a single `assert` map into an [`Assertion`], parsing its expression.
/// A parse failure is reported as S19 (pointing at the failing token within the
/// `assert` string) and leaves `expr` as `None`, mirroring the S04 handling of a
/// bad `join`. Returns `None` only for a node without a string `assert` value,
/// which the schema rejects upstream.
fn lower_assertion(node: &YamlWithSourceInfo, problems: &mut ProblemSet) -> Option<Assertion> {
    let entries = node.as_hash()?;
    let assert_entry = entries
        .iter()
        .find(|e| e.key.yaml.as_str() == Some("assert"))?;
    let text = assert_entry.value.yaml.as_str()?;
    let description = node
        .get_hash_value("description")
        .and_then(|d| d.yaml.as_str())
        .map(str::to_string);
    let span = assert_entry.value_span.clone();

    let expr = match AssertExpr::parse(text) {
        Ok(expr) => Some(expr),
        Err(err) => {
            let at = err.at.min(text.len());
            let sub = subspan(&span, at, at).unwrap_or_else(|| span.clone());
            problems.push(Problem::spec(
                "S19",
                Severity::Error,
                format!("`assert` expression does not parse: {}", err.message),
                sub,
            ));
            None
        }
    };

    Some(Assertion {
        text: Spanned::new(text.to_string(), span),
        expr,
        description,
    })
}

/// Lower a single `definitions` entry into a [`Definition`], parsing its
/// expression. A parse failure is reported as S19 (pointing at the failing
/// token within the `expr` string) and leaves `expr` as `None`, mirroring
/// [`lower_assertion`]. Returns `None` for a node without a string `expr`
/// value, which the schema rejects upstream.
fn lower_definition(node: &YamlWithSourceInfo, problems: &mut ProblemSet) -> Option<Definition> {
    let entries = node.as_hash()?;
    let name_entry = entries
        .iter()
        .find(|e| e.key.yaml.as_str() == Some("name"))?;
    // An empty/null name is kept (as "") so S11 can report it; the parser
    // collapses an empty name to null.
    let name = name_entry.value.yaml.as_str().unwrap_or("");
    let expr_entry = entries
        .iter()
        .find(|e| e.key.yaml.as_str() == Some("expr"))?;
    let text = expr_entry.value.yaml.as_str()?;
    let span = expr_entry.value_span.clone();

    let string = |key: &str| {
        entries
            .iter()
            .find(|e| e.key.yaml.as_str() == Some(key))
            .and_then(|e| e.value.yaml.as_str())
            .map(str::to_string)
    };

    let expr = match AssertExpr::parse(text) {
        Ok(expr) => Some(expr),
        Err(err) => {
            let at = err.at.min(text.len());
            let sub = subspan(&span, at, at).unwrap_or_else(|| span.clone());
            problems.push(Problem::spec(
                "S19",
                Severity::Error,
                format!("definition expression does not parse: {}", err.message),
                sub,
            ));
            None
        }
    };

    let todo = entries
        .iter()
        .find(|e| e.key.yaml.as_str() == Some("todo"))
        .and_then(|e| lower_todo(&e.value, e.value_span.clone()));

    Some(Definition {
        name: Spanned::new(name.to_string(), name_entry.value_span.clone()),
        text: Spanned::new(text.to_string(), span),
        expr,
        label: string("label"),
        description: string("description"),
        details: string("details"),
        todo,
    })
}

/// Lower an enum's `values` node into its allowed scalars with spans, and the
/// label each was written with. The labels are empty unless every value has
/// one, which keeps them index-aligned with the values they describe.
fn lower_enum_values(node: &YamlWithSourceInfo) -> (Vec<Spanned<Scalar>>, Vec<String>) {
    let Some(entries) = node.as_hash() else {
        // List form (or a lone scalar, which the schema rejects upstream).
        return (lower_scalars(node), Vec::new());
    };
    // Map form: the keys are the values, and each carries its label.
    let items = entries
        .iter()
        .map(|entry| Spanned::new(lower_scalar(&entry.key), entry.key.source_info.clone()))
        .collect();
    let labels: Option<Vec<String>> = entries
        .iter()
        .map(|entry| match lower_scalar(&entry.value) {
            Scalar::String(label) => Some(label),
            // The schema rejects a non-string label upstream; a partial set
            // would break the alignment, so none are kept.
            _ => None,
        })
        .collect();
    (items, labels.unwrap_or_default())
}

/// Lower a `range` or `examples` node into its scalar elements with spans.
/// Non-array nodes yield an empty vector (the schema rejects them upstream).
fn lower_scalars(node: &YamlWithSourceInfo) -> Vec<Spanned<Scalar>> {
    let Some(items) = node.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| Spanned::new(lower_scalar(item), item.source_info.clone()))
        .collect()
}

fn lower_scalar(node: &YamlWithSourceInfo) -> Scalar {
    let yaml = &node.yaml;
    if let Some(b) = yaml.as_bool() {
        Scalar::Bool(b)
    } else if let Some(i) = yaml.as_i64() {
        Scalar::Int(i)
    } else if let Some(f) = yaml.as_f64() {
        Scalar::Float(f)
    } else if let Some(s) = yaml.as_str() {
        Scalar::String(s.to_string())
    } else if node.as_array().is_some() || node.as_hash().is_some() {
        Scalar::Compound
    } else {
        Scalar::Null
    }
}

fn lower_relationship(node: &YamlWithSourceInfo, problems: &mut ProblemSet) -> Relationship {
    let entries = node.as_hash().expect("schema guarantees mapping");
    let mut description: Option<String> = None;
    let mut cardinality: Option<Spanned<Cardinality>> = None;
    let mut join_text: Option<Spanned<String>> = None;
    let mut conflicts: Vec<Spanned<String>> = Vec::new();
    let mut aliases: Vec<Alias> = Vec::new();
    let mut todo: Option<Spanned<String>> = None;

    for entry in entries {
        let Some(key) = entry.key.yaml.as_str() else {
            continue;
        };
        match key {
            "description" => description = entry.value.yaml.as_str().map(str::to_string),
            "cardinality" => {
                if let Some(s) = entry.value.yaml.as_str()
                    && let Some(c) = Cardinality::parse(s)
                {
                    cardinality = Some(Spanned::new(c, entry.value_span.clone()));
                }
            }
            "join" => {
                if let Some(s) = entry.value.yaml.as_str() {
                    join_text = Some(Spanned::new(s.to_string(), entry.value_span.clone()));
                }
            }
            "conflicts" => {
                if let Some(items) = entry.value.as_array() {
                    for c in items {
                        if let Some(s) = c.yaml.as_str() {
                            conflicts.push(Spanned::new(s.to_string(), c.source_info.clone()));
                        }
                    }
                }
            }
            "aliases" => {
                if let Some(items) = entry.value.as_hash() {
                    for a in items {
                        let (Some(name), Some(table)) =
                            (a.key.yaml.as_str(), a.value.yaml.as_str())
                        else {
                            continue;
                        };
                        aliases.push(Alias {
                            name: Spanned::new(name.to_string(), a.key.source_info.clone()),
                            table: Spanned::new(table.to_string(), a.value_span.clone()),
                        });
                    }
                }
            }
            "todo" => {
                todo = lower_todo(&entry.value, entry.value_span.clone());
            }
            _ => {}
        }
    }

    let cardinality = cardinality.expect("schema guarantees cardinality is a valid enum");
    let join_text = join_text.expect("schema guarantees join is present and a string");

    let join = match JoinExpr::parse(&join_text.value) {
        Ok(expr) => Some(expr),
        Err(err) => {
            let span =
                crate::problem::subspan(&join_text.span, err.at, err.at.min(join_text.value.len()))
                    .unwrap_or_else(|| join_text.span.clone());
            problems.push(Problem::spec(
                "S04",
                Severity::Error,
                format!("`join` expression does not parse: {}", err.message),
                span,
            ));
            None
        }
    };

    Relationship {
        description,
        cardinality,
        join_text,
        join,
        conflicts,
        aliases,
        todo,
    }
}
