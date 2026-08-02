//! Typed in-memory model of a data dictionary.
//!
//! Lowered from the source YAML by `lower::lower` once the structural schema
//! has accepted the document, so the lowering code can assume well-formed
//! input. Each significant node carries a `SourceInfo` so schema-check diagnostics
//! can point back at the source.

use quarto_source_map::SourceInfo;

use crate::assert_expr::AssertExpr;
use crate::join_expr::JoinExpr;

#[derive(Debug, Clone)]
pub struct Spanned<T> {
    pub value: T,
    pub span: SourceInfo,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: SourceInfo) -> Self {
        Self { value, span }
    }
}

#[derive(Debug, Clone)]
pub struct DataDict {
    pub tables: Vec<Table>,
    pub relationships: Vec<Relationship>,
}

impl DataDict {
    /// The first table with the given name, or `None`. Duplicate names are an
    /// error (S10); lookups resolve to the first so downstream checks still run.
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name.value == name)
    }

    /// The `(table, column)` a single-column foreign key points at: the
    /// `primary_key` on the other side of a relationship whose join names `col`.
    /// `None` if `col` is not a foreign key, or no relationship resolves it (the
    /// S01 case). Shared by the S01 spec check and the D05/D06 data checks.
    pub fn resolve_foreign_key(&self, table: &Table, col: &Column) -> Option<(&Table, &Column)> {
        if !col.has(Constraint::ForeignKey) {
            return None;
        }
        let table_name = table.name.value.as_str();
        for rel in &self.relationships {
            let Some(join) = &rel.join else { continue };
            for conj in &join.conjuncts {
                for (fk_side, pk_side) in [(&conj.lhs, &conj.rhs), (&conj.rhs, &conj.lhs)] {
                    if rel.resolve(&fk_side.table) != table_name || fk_side.column != col.name.value
                    {
                        continue;
                    }
                    let Some(other_tbl) = self.table(rel.resolve(&pk_side.table)) else {
                        continue;
                    };
                    let Some(other_col) = other_tbl.column(&pk_side.column) else {
                        continue;
                    };
                    if other_col.has(Constraint::PrimaryKey) {
                        return Some((other_tbl, other_col));
                    }
                }
            }
        }
        None
    }
}

/// A table- or column-level `assert` constraint: the expression text with its
/// span, the parsed form (`None` if it failed to parse — S19 is emitted then),
/// and an optional description. Mirrors how [`Relationship`] holds both the
/// `join` text and its parsed `JoinExpr`.
#[derive(Debug, Clone)]
pub struct Assertion {
    pub text: Spanned<String>,
    pub expr: Option<AssertExpr>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub name: Spanned<String>,
    pub columns: Vec<Column>,
    /// Table-level assertions (span multiple columns).
    pub constraints: Vec<Assertion>,
    /// Where the table's data lives, when it declares a `source`. Optional
    /// for spec validation; required for metadata validation (M04).
    pub source: Option<Source>,
    /// Spans of the `label`/`description`/`details` keys, when present. Held so
    /// S16 can point at a single-table dictionary's misplaced table-level
    /// descriptions.
    pub label: Option<SourceInfo>,
    pub description: Option<SourceInfo>,
    pub details: Option<SourceInfo>,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub span: SourceInfo,
    /// path relative to dictionary
    pub parquet: Spanned<String>,
}

impl Table {
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name.value == name)
    }
}

#[derive(Debug, Clone)]
pub struct Column {
    pub name: Spanned<String>,
    pub constraints: Vec<Spanned<Constraint>>,
    /// Column-level `assert` constraints (the map form of a `constraints` entry).
    pub assertions: Vec<Assertion>,
    pub col_type: Option<Spanned<String>>,
    /// The allowed values of an `enum` column: the list items, or the keys of
    /// the map form (whose labels are dropped — only the values are constrained).
    pub values: Option<Representation>,
    pub range: Option<Representation>,
    pub examples: Option<Representation>,
    pub units: Option<Spanned<String>>,
    pub time_zone: Option<Spanned<String>>,
}

#[derive(Debug, Clone)]
pub struct Representation {
    /// The value node — the list, map, or whatever stands in for one.
    pub span: SourceInfo,
    /// The `values` / `range` / `examples` key itself, for showing the line a
    /// finding sits under.
    pub key_span: SourceInfo,
    pub items: Vec<Spanned<Scalar>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    /// An integer, kept distinct from `Float` so an exact value survives;
    /// routing every number through `f64` would lose precision past 2^53.
    Int(i64),
    Float(f64),
    String(String), // includes date/times
    Bool(bool),
    Null,
    /// A list or map — never valid in a representation list.
    Compound,
}

impl Scalar {
    /// English noun phrase naming the scalar's kind, for diagnostics.
    pub fn noun(&self) -> &'static str {
        match self {
            Scalar::Int(_) | Scalar::Float(_) => "a number",
            Scalar::String(_) => "a string",
            Scalar::Bool(_) => "a boolean",
            Scalar::Null => "null",
            Scalar::Compound => "a list or map",
        }
    }

    /// The numeric value as `f64` for ordering comparisons (S13 range order),
    /// or `None` if not a number.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Scalar::Int(n) => Some(*n as f64),
            Scalar::Float(n) => Some(*n),
            _ => None,
        }
    }

    /// The string form this value takes in data, for enum membership (D04).
    /// `None` for anything but a string, since an `enum`'s values are strings
    /// (S24) and its underlying column is string-like.
    pub fn as_enum_value(&self) -> Option<&str> {
        match self {
            Scalar::String(s) => Some(s),
            _ => None,
        }
    }
}

impl Column {
    pub fn has(&self, c: Constraint) -> bool {
        self.constraints.iter().any(|x| x.value == c)
    }

    /// True if the column is unique-by-row: explicitly `unique` or
    /// `primary_key` (which the spec defines as implying `unique`).
    pub fn is_unique_implied(&self) -> bool {
        self.has(Constraint::Unique) || self.has(Constraint::PrimaryKey)
    }

    /// True if the column may not contain nulls: explicitly `required` or
    /// `primary_key` (which the spec defines as implying `required`).
    pub fn is_required_implied(&self) -> bool {
        self.has(Constraint::Required) || self.has(Constraint::PrimaryKey)
    }

    pub fn is_enum(&self) -> bool {
        self.col_type.as_ref().is_some_and(|t| t.value == "enum")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Constraint {
    PrimaryKey,
    ForeignKey,
    Required,
    Unique,
}

impl Constraint {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "primary_key" => Self::PrimaryKey,
            "foreign_key" => Self::ForeignKey,
            "required" => Self::Required,
            "unique" => Self::Unique,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Relationship {
    pub cardinality: Spanned<Cardinality>,
    /// The original join string with its source span. Kept alongside the
    /// parsed `JoinExpr` so diagnostics about parse failure can refer back to
    /// it.
    pub join_text: Spanned<String>,
    /// `None` if the join string failed to parse — S04 is emitted in that
    /// case and downstream rules that need the parsed form (S01, S05,
    /// S06) skip the relationship.
    pub join: Option<JoinExpr>,
    pub conflicts: Vec<Spanned<String>>,
    /// Alias declarations, in source order. Scoped to this relationship.
    pub aliases: Vec<Alias>,
}

impl Relationship {
    /// The table a name in the `join` refers to: the target of the alias of
    /// that name, or `name` itself when no alias declares it.
    pub fn resolve<'a>(&'a self, name: &'a str) -> &'a str {
        self.alias(name).map_or(name, |a| a.table.value.as_str())
    }

    pub fn alias(&self, name: &str) -> Option<&Alias> {
        self.aliases.iter().find(|a| a.name.value == name)
    }
}

/// One `aliases` entry: the alias itself and the table it stands for.
#[derive(Debug, Clone)]
pub struct Alias {
    pub name: Spanned<String>,
    pub table: Spanned<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    OneToOne,
    OneToMany,
    ManyToOne,
}

impl Cardinality {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "one-to-one" => Self::OneToOne,
            "one-to-many" => Self::OneToMany,
            "many-to-one" => Self::ManyToOne,
            _ => return None,
        })
    }
}
