//! Draft a starting data dictionary from parquet files.
//!
//! [`draft`] profiles each input file (see [`data_dict_parquet::profile`]) and
//! generates one table entry per file: inferred types, observed ranges and
//! examples, and a `# TODO:` comment for everything only a human can decide —
//! descriptions, enum candidates, constraints, the primary key. The output is
//! always spec-valid, so the TODOs can be worked through incrementally under
//! `validate-spec`.
//!
//! With no existing dictionary a complete file is generated; with one, new
//! tables are appended after the last entry in `tables` and every existing
//! byte is preserved verbatim.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate};
use data_dict_parquet::{
    ColumnProfile, Distinct, ParquetError, TimeGrain, Value, ValueKind, profile,
};

use crate::validate_spec::{LEARN_MORE_URL, SPEC_VERSION, load_str};

/// Target line width for generated YAML and wrapped comments.
const WIDTH: usize = 80;

/// Relative tolerance when comparing an approximate distinct count against the
/// row count: the profiler's HyperLogLog sketch has a standard error of ~2%.
const APPROX_TOLERANCE: f64 = 0.02;

/// Fewest repeats per value before a string column reads as an enum candidate:
/// at most 12 distinct values, each value appearing at least twice on average.
const ENUM_MAX_VALUES: usize = 12;

/// The placeholder descriptions. They are real `description:` keys, not
/// comments, so the questions travel with the file until answered; the `TODO:`
/// prefix keeps them greppable.
const DATASET_DESCRIPTION_TODO: &str = "TODO: describe the dataset as a whole.";
const TABLE_DESCRIPTION_TODO: &str =
    "TODO: what's the grain? what's the population? how was the data collected?";
const COLUMN_DESCRIPTION_TODO: &str = "TODO: what does this column mean?";

#[derive(Debug)]
pub struct DraftOutcome {
    /// The complete new contents for the dictionary file.
    pub content: String,
    /// Names of the tables that were added, in output order.
    pub added: Vec<String>,
    /// Names of the input tables skipped because the dictionary already has
    /// a table of that name.
    pub skipped: Vec<String>,
}

#[derive(Debug)]
pub enum DraftError {
    /// Two inputs share a file stem, so they would produce the same table name.
    DuplicateStem {
        stem: String,
        first: PathBuf,
        second: PathBuf,
    },
    /// An input has no file stem to name its table after.
    NoStem {
        path: PathBuf,
    },
    /// An input could not be opened or profiled as parquet.
    Parquet {
        path: PathBuf,
        source: ParquetError,
    },
    /// The existing dictionary can't be appended to safely because it doesn't
    /// parse or doesn't match the schema.
    ExistingInvalid {
        message: String,
    },
    /// The existing dictionary declares `tables` but the list is empty, so
    /// there is no entry to splice after.
    EmptyTables,
    Io(std::io::Error),
}

impl fmt::Display for DraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DraftError::DuplicateStem {
                stem,
                first,
                second,
            } => write!(
                f,
                "two inputs would both be named `{stem}`: {} and {}",
                first.display(),
                second.display()
            ),
            DraftError::NoStem { path } => {
                write!(
                    f,
                    "{}: no file name to derive a table name from",
                    path.display()
                )
            }
            DraftError::Parquet { path, source } => write!(f, "{}: {source}", path.display()),
            DraftError::ExistingInvalid { message } => write!(
                f,
                "can't append to the existing dictionary: {message} (fix it with `data-dict validate-spec`, or choose another --output)"
            ),
            DraftError::EmptyTables => write!(
                f,
                "the existing dictionary has an empty `tables` list; remove it (or add a first table) and re-run"
            ),
            DraftError::Io(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for DraftError {}

/// Draft a dictionary for `inputs`, one table per file, named after its stem.
/// `output_dir` is where the dictionary will live — `source.parquet` paths are
/// written relative to it. `existing` is the current contents of the output
/// file (`None` to generate a complete new file).
pub fn draft(
    inputs: &[PathBuf],
    output_dir: &Path,
    existing: Option<&str>,
) -> Result<DraftOutcome, DraftError> {
    let named = name_inputs(inputs)?;
    match existing {
        None => {
            let tables = draft_tables(&named, output_dir)?;
            Ok(DraftOutcome {
                content: emit_new_file(&tables),
                added: tables.into_iter().map(|t| t.name).collect(),
                skipped: Vec::new(),
            })
        }
        Some(text) => append(text, &named, output_dir),
    }
}

/// Pair each input with the table name its stem gives, rejecting collisions
/// before anything is profiled or written.
fn name_inputs(inputs: &[PathBuf]) -> Result<Vec<(String, &Path)>, DraftError> {
    let mut seen: HashMap<String, &Path> = HashMap::new();
    let mut named = Vec::new();
    for path in inputs {
        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            return Err(DraftError::NoStem { path: path.clone() });
        };
        if let Some(first) = seen.get(&stem) {
            return Err(DraftError::DuplicateStem {
                stem,
                first: first.to_path_buf(),
                second: path.clone(),
            });
        }
        seen.insert(stem.clone(), path);
        named.push((stem, path.as_path()));
    }
    Ok(named)
}

fn draft_tables(
    named: &[(String, &Path)],
    output_dir: &Path,
) -> Result<Vec<DraftTable>, DraftError> {
    named
        .iter()
        .map(|(name, path)| {
            let profile = profile(path, None).map_err(|source| DraftError::Parquet {
                path: path.to_path_buf(),
                source,
            })?;
            let source = relative_to(path, output_dir).map_err(DraftError::Io)?;
            Ok(infer_table(name.clone(), &source, &profile))
        })
        .collect()
}

// --- inference ---------------------------------------------------------

struct DraftTable {
    name: String,
    /// The `source.parquet` path, relative to the dictionary.
    source: String,
    columns: Vec<DraftColumn>,
}

struct DraftColumn {
    name: String,
    /// `None` leaves the column name-only: the profiler couldn't read it, so
    /// the draft acknowledges it without claiming anything.
    dict_type: Option<&'static str>,
    /// The one combined `constraints:` recommendation, placed right after
    /// `type:` — where a real `constraints` key would go. Its stub lines are
    /// the literal YAML to uncomment when the observation checks out.
    constraints_todo: Option<Todo>,
    time_zone: Option<&'static str>,
    /// Pre-rendered YAML scalars for `range: [min, max]`.
    range: Option<[String; 2]>,
    /// Pre-rendered YAML scalars for `examples:`.
    examples: Option<Vec<String>>,
    todos: Vec<Todo>,
}

/// One `# TODO:` annotation: the prose, plus commented-out YAML lines
/// suggesting what to write (e.g. an enum's `values`).
struct Todo {
    text: String,
    stub: Vec<String>,
}

impl Todo {
    fn new(text: impl Into<String>) -> Self {
        Todo {
            text: text.into(),
            stub: Vec::new(),
        }
    }
}

fn infer_table(name: String, source: &str, file: &data_dict_parquet::FileProfile) -> DraftTable {
    let rows = file.row_count;
    let columns: Vec<DraftColumn> = file.columns.iter().map(|c| infer_column(c, rows)).collect();
    DraftTable {
        name,
        source: source.to_string(),
        columns,
    }
}

/// `Some(approximate)` when every value of the column looks distinct — the
/// grounds for suggesting `unique` and nominating a primary-key candidate.
fn distinctness(col: &ColumnProfile, rows: usize) -> Option<bool> {
    if rows == 0 {
        return None;
    }
    match col.distinct? {
        // An exact count is definitive either way. `distinct == rows` also
        // implies there were no nulls, since nulls are never counted as values.
        Distinct::Exact(d) => (d == rows).then_some(false),
        // An approximate count within the sketch's error of the row count only
        // suggests distinctness, and only when no nulls hide missing rows.
        Distinct::Approx(d) => {
            let close = (d as f64 - rows as f64).abs() <= APPROX_TOLERANCE * rows as f64;
            (close && col.null_count == Some(0)).then_some(true)
        }
    }
}

fn infer_column(col: &ColumnProfile, rows: usize) -> DraftColumn {
    let dict_type = dict_type(&col.kind, &col.name);
    let mut todos = Vec::new();

    if let ValueKind::Unsupported(reason) = col.kind {
        todos.push(Todo::new(format!(
            "this column could not be profiled ({reason}); describe it and add a type."
        )));
    }

    if let Some(values) = enum_candidate(col, rows) {
        // Accepting this one isn't a pure uncomment: an enum column swaps its
        // `examples` for `values` (spec rule S07), so the comment spells that
        // out.
        let mut todo = Todo::new(format!(
            "only {} distinct values — if this is an enum, set type: enum and \
             replace examples with:",
            values.len()
        ));
        let rendered: Vec<String> = values.iter().map(|v| yaml_scalar(v)).collect();
        todo.stub = stub_list("values", &rendered);
        todos.push(todo);
    }

    // One combined recommendation per column: what was observed, then the
    // literal `constraints:` line(s) to uncomment (or delete, if the
    // observation doesn't hold beyond this file). A column whose values are
    // all distinct necessarily has no nulls, so the all-distinct case offers
    // the unique/primary-key pair rather than a bare `required`.
    let constraints_todo = match distinctness(col, rows) {
        Some(approx) => {
            let mut todo = Todo::new(if approx {
                "values look distinct (approximate count), none missing — \
                 uncomment one or delete:"
            } else {
                "all values distinct, none missing — uncomment one or delete:"
            });
            todo.stub
                .push("constraints: [required, unique]".to_string());
            todo.stub.push("constraints: [primary_key]".to_string());
            Some(todo)
        }
        None if rows > 0 && col.null_count == Some(0) => {
            let mut todo = Todo::new("no missing values observed — uncomment or delete:");
            todo.stub.push("constraints: [required]".to_string());
            Some(todo)
        }
        None => None,
    };

    let range = matches!(col.kind, ValueKind::Date | ValueKind::Timestamp { .. })
        .then(|| render_range(col))
        .flatten();
    let examples = matches!(dict_type, Some("string" | "number" | "number(id)"))
        .then(|| col.examples.iter().map(render_plain).collect::<Vec<_>>());
    let time_zone = match col.kind {
        ValueKind::Timestamp {
            utc_adjusted: true, ..
        } => Some("UTC"),
        ValueKind::Timestamp {
            utc_adjusted: false,
            ..
        } => Some("naive"),
        _ => None,
    };

    DraftColumn {
        name: col.name.clone(),
        dict_type,
        constraints_todo,
        time_zone,
        range,
        examples,
        todos,
    }
}

/// The data-dict type a profiled column drafts as, `None` for a column the
/// profiler couldn't read (drafted name-only, which the spec never checks).
/// Follows the same mapping as `data-dict types parquet`.
fn dict_type(kind: &ValueKind, name: &str) -> Option<&'static str> {
    Some(match kind {
        ValueKind::Bool => "boolean",
        ValueKind::Int | ValueKind::Float if id_like(name) => "number(id)",
        ValueKind::Int | ValueKind::Float => "number",
        // A time of day has no dedicated data-dict type; its raw value is
        // numeric, matching the `types parquet` mapping.
        ValueKind::Time { .. } => "number",
        ValueKind::Text => "string",
        ValueKind::Date => "date",
        ValueKind::Timestamp { .. } => "datetime",
        ValueKind::Unsupported(_) => return None,
    })
}

fn id_like(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "id" || name.ends_with("_id")
}

/// The observed values of a string column that reads as an enum: few enough
/// distinct values, each clearly repeated. Requires an exact distinct count —
/// with at most [`ENUM_MAX_VALUES`] values the tracker never overflows, so an
/// approximate count already rules a column out.
fn enum_candidate(col: &ColumnProfile, rows: usize) -> Option<Vec<String>> {
    if col.kind != ValueKind::Text {
        return None;
    }
    let Some(Distinct::Exact(d)) = col.distinct else {
        return None;
    };
    if d == 0 || d > ENUM_MAX_VALUES || 2 * d >= rows {
        return None;
    }
    let mut values: Vec<String> = col
        .value_counts
        .iter()
        .filter_map(|vc| match &vc.value {
            Value::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    // The tracker holds more values than ENUM_MAX_VALUES, so all d are here.
    debug_assert_eq!(values.len(), d);
    values.sort();
    Some(values)
}

/// Render a temporal column's observed `[min, max]`; an unobserved end is left
/// open with `-.inf` / `.inf` (spec rule S12).
fn render_range(col: &ColumnProfile) -> Option<[String; 2]> {
    let render = |v: &Value| render_temporal(v, &col.kind);
    let min = col.min.as_ref().and_then(render);
    let max = col.max.as_ref().and_then(render);
    if min.is_none() && max.is_none() {
        return None;
    }
    Some([
        min.unwrap_or_else(|| "-.inf".to_string()),
        max.unwrap_or_else(|| ".inf".to_string()),
    ])
}

// --- value rendering -----------------------------------------------------

/// Render a value for `examples:` — a bare number, or a quoted-as-needed
/// string.
fn render_plain(value: &Value) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.get().to_string(),
        Value::Text(s) => yaml_scalar(s),
    }
}

/// Render a date or timestamp value as the ISO 8601 string its column's
/// `range` wants. Timestamps are zoneless: a drafted `datetime` column always
/// carries a `time_zone`, and the spec writes such values without an offset.
/// `None` when the raw number falls outside the representable range.
fn render_temporal(value: &Value, kind: &ValueKind) -> Option<String> {
    let &Value::Int(raw) = value else {
        return None;
    };
    match kind {
        ValueKind::Date => {
            let date = NaiveDate::from_num_days_from_ce_opt(
                i32::try_from(raw).ok()? + days_from_ce_to_epoch(),
            )?;
            Some(date.format("%Y-%m-%d").to_string())
        }
        ValueKind::Timestamp { grain, .. } => {
            let dt = match grain {
                TimeGrain::Millis => DateTime::from_timestamp_millis(raw)?,
                TimeGrain::Micros => DateTime::from_timestamp_micros(raw)?,
                TimeGrain::Nanos => DateTime::from_timestamp_nanos(raw),
            }
            .naive_utc();
            let format = if dt.and_utc().timestamp_subsec_nanos() == 0 {
                "%Y-%m-%dT%H:%M:%S"
            } else {
                "%Y-%m-%dT%H:%M:%S%.f"
            };
            Some(dt.format(format).to_string())
        }
        _ => None,
    }
}

/// Days from `NaiveDate`'s internal epoch (0001-01-01) to the Unix epoch.
fn days_from_ce_to_epoch() -> i32 {
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .expect("the Unix epoch is a valid date")
        .signed_duration_since(NaiveDate::from_ymd_opt(1, 1, 1).expect("day one is a valid date"))
        .num_days() as i32
        + 1
}

// --- YAML scalars ----------------------------------------------------------

/// Render a string as a YAML scalar, quoting whenever a plain scalar could be
/// misread. The rules are deliberately conservative — quoting is always safe —
/// but leave everyday identifiers unquoted. Values are emitted inside flow
/// sequences too, so flow indicators (`[]{},`) always force quotes.
fn yaml_scalar(s: &str) -> String {
    if needs_quoting(s) {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\r' => out.push_str("\\r"),
                c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    } else {
        s.to_string()
    }
}

fn needs_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    let first = s.chars().next().expect("non-empty");
    let last = s.chars().next_back().expect("non-empty");
    if first.is_whitespace() || last.is_whitespace() {
        return true;
    }
    // Characters with syntactic meaning somewhere in YAML: indicators, flow
    // syntax, comments, quotes, escapes. `:` only bites before a space or at
    // the end, but quoting it everywhere costs little.
    if s.chars().any(|c| {
        matches!(
            c,
            ':' | '#'
                | ','
                | '['
                | ']'
                | '{'
                | '}'
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '%'
                | '@'
                | '`'
                | '"'
                | '\''
                | '\\'
        ) || c.is_control()
    }) {
        return true;
    }
    if matches!(first, '-' | '?') && (s.len() == 1 || s.as_bytes()[1] == b' ') {
        return true;
    }
    reads_as_non_string(s)
}

/// Whether a plain scalar would parse as something other than a string —
/// exactly the values spec rule S12 requires quoted in string `examples` and
/// enum `values`.
fn reads_as_non_string(s: &str) -> bool {
    if matches!(
        s,
        "~" | "null"
            | "Null"
            | "NULL"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | "yes"
            | "Yes"
            | "YES"
            | "no"
            | "No"
            | "NO"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
    ) {
        return true;
    }
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    if matches!(body, ".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    if let Some(hex) = body.strip_prefix("0x")
        && !hex.is_empty()
        && hex.chars().all(|c| c.is_ascii_hexdigit())
    {
        return true;
    }
    if let Some(oct) = body.strip_prefix("0o")
        && !oct.is_empty()
        && oct.chars().all(|c| ('0'..='7').contains(&c))
    {
        return true;
    }
    // Everything else numeric — ints, floats, exponents, leading dots — is
    // what `f64` accepts.
    s.parse::<f64>().is_ok()
}

// --- emission ----------------------------------------------------------

/// A complete new dictionary: preamble, dataset description, and every table.
fn emit_new_file(tables: &[DraftTable]) -> String {
    let mut out = String::new();
    out.push_str(&format!("$version: {SPEC_VERSION}\n"));
    out.push_str(&format!("$learn_more: {LEARN_MORE_URL}\n"));
    out.push('\n');
    emit_description(&mut out, "", DATASET_DESCRIPTION_TODO);
    out.push('\n');
    out.push_str("tables:\n");
    for (i, table) in tables.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // A single-table dictionary describes itself at the top level (S16
        // warns against a table-level description there).
        emit_table(&mut out, table, "  - ", tables.len() > 1);
    }
    out
}

/// One `- name: …` table entry. `item_prefix` is everything before `name` on
/// the first line (e.g. `"  - "`); the entry's other lines are indented to its
/// width, so appended entries can match whatever indent the file already uses.
fn emit_table(out: &mut String, table: &DraftTable, item_prefix: &str, with_description: bool) {
    let indent = " ".repeat(item_prefix.len());
    out.push_str(item_prefix);
    out.push_str(&format!("name: {}\n", yaml_scalar(&table.name)));
    out.push_str(&format!("{indent}source:\n"));
    out.push_str(&format!(
        "{indent}  parquet: {}\n",
        yaml_scalar(&table.source)
    ));
    if with_description {
        emit_description(out, &indent, TABLE_DESCRIPTION_TODO);
    }
    out.push_str(&format!("{indent}columns:\n"));
    let col_prefix = format!("{indent}  - ");
    for column in &table.columns {
        emit_column(out, column, &col_prefix);
    }
}

fn emit_column(out: &mut String, column: &DraftColumn, item_prefix: &str) {
    let indent = " ".repeat(item_prefix.len());
    out.push_str(item_prefix);
    out.push_str(&format!("name: {}\n", yaml_scalar(&column.name)));
    if let Some(dict_type) = column.dict_type {
        out.push_str(&format!("{indent}type: {dict_type}\n"));
    }
    if let Some(todo) = &column.constraints_todo {
        emit_todo(out, &indent, todo);
    }
    if let Some(time_zone) = column.time_zone {
        out.push_str(&format!("{indent}time_zone: {time_zone}\n"));
    }
    emit_description(out, &indent, COLUMN_DESCRIPTION_TODO);
    if let Some([min, max]) = &column.range {
        out.push_str(&format!("{indent}range: [{min}, {max}]\n"));
    }
    if let Some(examples) = &column.examples {
        emit_list(out, &indent, "examples", examples);
    }
    for todo in &column.todos {
        emit_todo(out, &indent, todo);
    }
}

/// A `# TODO:` comment (wrapped) followed by its commented-out YAML stub.
fn emit_todo(out: &mut String, indent: &str, todo: &Todo) {
    wrap_comment(out, indent, &format!("TODO: {}", todo.text));
    for line in &todo.stub {
        out.push_str(&format!("{indent}# {line}\n"));
    }
}

/// A `description: >` folded block scalar at `indent`, with `text` wrapped to
/// [`WIDTH`] on the following lines. Folding joins the wrapped lines back into
/// one paragraph when the YAML is read.
fn emit_description(out: &mut String, indent: &str, text: &str) {
    out.push_str(&format!("{indent}description: >\n"));
    let inner = format!("{indent}  ");
    let available = WIDTH.saturating_sub(inner.len()).max(20);
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > available {
            out.push_str(&format!("{inner}{line}\n"));
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push_str(&format!("{inner}{line}\n"));
    }
}

/// A `key: [a, b, c]` line, falling back to a block sequence (one item per
/// line) when the flow form would overrun [`WIDTH`]. A single long scalar is
/// never broken mid-value — it just gets its own long line.
fn emit_list(out: &mut String, indent: &str, key: &str, items: &[String]) {
    let flow = format!("{indent}{key}: [{}]", items.join(", "));
    if flow.len() <= WIDTH {
        out.push_str(&flow);
        out.push('\n');
    } else {
        out.push_str(&format!("{indent}{key}:\n"));
        for item in items {
            out.push_str(&format!("{indent}  - {item}\n"));
        }
    }
}

/// The commented-out stub lines for a suggested `key: [a, b, c]`, with the
/// same flow-to-block fallback as [`emit_list`] (the `# ` prefix counts
/// against the width via the caller's indent — close enough for a comment).
fn stub_list(key: &str, items: &[String]) -> Vec<String> {
    let flow = format!("{key}: [{}]", items.join(", "));
    if flow.len() <= WIDTH.saturating_sub(10) {
        vec![flow]
    } else {
        let mut lines = vec![format!("{key}:")];
        lines.extend(items.iter().map(|item| format!("  - {item}")));
        lines
    }
}

/// Emit `text` as `# `-prefixed comment lines at `indent`, wrapped at word
/// boundaries to stay within [`WIDTH`].
fn wrap_comment(out: &mut String, indent: &str, text: &str) {
    let available = WIDTH.saturating_sub(indent.len() + 2).max(20);
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > available {
            out.push_str(&format!("{indent}# {line}\n"));
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push_str(&format!("{indent}# {line}\n"));
    }
}

// --- paths ---------------------------------------------------------------

/// `path` relative to `dir`, computed lexically (no filesystem access beyond
/// making both absolute against the current directory).
fn relative_to(path: &Path, dir: &Path) -> Result<String, std::io::Error> {
    let path = std::path::absolute(path)?;
    let dir = std::path::absolute(if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    })?;
    let mut path_components = path.components().peekable();
    let mut dir_components = dir.components().peekable();
    while path_components.peek().is_some() && path_components.peek() == dir_components.peek() {
        path_components.next();
        dir_components.next();
    }
    let mut rel = PathBuf::new();
    for _ in dir_components {
        rel.push("..");
    }
    for component in path_components {
        rel.push(component);
    }
    Ok(rel.display().to_string())
}

// --- append mode -----------------------------------------------------------

fn append(
    existing: &str,
    named: &[(String, &Path)],
    output_dir: &Path,
) -> Result<DraftOutcome, DraftError> {
    let (mut problems, doc) = load_str(existing, "data-dict.yaml").map_err(|problems| {
        let message = problems
            .items
            .first()
            .map(|p| p.message.clone())
            .unwrap_or_else(|| "it does not parse".to_string());
        DraftError::ExistingInvalid { message }
    })?;
    let dict = crate::lower::lower(&doc, &mut problems);

    let (fresh, skipped): (Vec<_>, Vec<_>) = named
        .iter()
        .cloned()
        .partition(|(name, _)| dict.table(name).is_none());
    let skipped: Vec<String> = skipped.into_iter().map(|(name, _)| name).collect();
    if fresh.is_empty() {
        return Ok(DraftOutcome {
            content: existing.to_string(),
            added: Vec::new(),
            skipped,
        });
    }
    let tables = draft_tables(&fresh, output_dir)?;
    // Same S16-driven rule as a new file, counting what's already there: only
    // a dictionary that ends up multi-table gets table-level descriptions.
    let with_description = dict.tables.len() + tables.len() > 1;

    let content =
        if doc.get_hash_value("tables").is_none() {
            // No `tables` yet: start the section at the end of the file.
            let mut content = existing.to_string();
            if !content.is_empty() && !content.ends_with('\n') {
                content.push('\n');
            }
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str("tables:\n");
            for (i, table) in tables.iter().enumerate() {
                if i > 0 {
                    content.push('\n');
                }
                emit_table(&mut content, table, "  - ", with_description);
            }
            content
        } else {
            let Some(last) = dict.tables.last() else {
                return Err(DraftError::EmptyTables);
            };
            let (point, item_prefix) = insertion_point(existing, last, &problems.source)
                .ok_or_else(|| DraftError::ExistingInvalid {
                    message: "the last table's location in the source can't be resolved"
                        .to_string(),
                })?;
            let mut inserted = String::new();
            for table in &tables {
                inserted.push('\n');
                emit_table(&mut inserted, table, &item_prefix, with_description);
            }
            // Keep a blank line between the appended entries and whatever follows
            // (a later `relationships:`/`glossary:` section), mirroring the blank
            // line inserted above them.
            let rest = &existing[point..];
            if !rest.is_empty() && !rest.starts_with('\n') {
                inserted.push('\n');
            }
            format!("{}{}{}", &existing[..point], inserted, rest)
        };

    Ok(DraftOutcome {
        content,
        added: tables.into_iter().map(|t| t.name).collect(),
        skipped,
    })
}

/// Where to splice new table entries, and the `- `-bearing prefix that puts
/// them at the same indent as the file's last table: the start of the line
/// after `last`'s node ends (so a same-line trailing comment stays attached).
fn insertion_point(
    content: &str,
    last: &crate::model::Table,
    source: &crate::SourceContext,
) -> Option<(usize, String)> {
    let span = &last.span;
    let start = span
        .map_offset(0, source)?
        .location
        .offset
        .min(content.len());
    let end = span
        .map_offset(span.length(), source)?
        .location
        .offset
        .min(content.len());
    // The node's span runs on through any blank lines to the next token, so
    // find the entry's real last byte, then step to the start of the next line
    // (skipping past any same-line trailing comment).
    let entry_end = start + content[start..end].trim_end().len();
    let point = match content[entry_end..].find('\n') {
        Some(i) => entry_end + i + 1,
        None => content.len(),
    };
    // The node starts at the entry's first key; the text before it on its line
    // is the `  - ` item prefix.
    let line_start = content[..start].rfind('\n').map_or(0, |i| i + 1);
    let before = &content[line_start..start];
    let item_prefix = if before.trim() == "-" {
        before.to_string()
    } else {
        "  - ".to_string()
    };
    Some((point, item_prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_identifiers_stay_unquoted() {
        for s in [
            "species",
            "bill_length_mm",
            "Adelie",
            "SE - whale bay",
            "a-b",
        ] {
            assert_eq!(yaml_scalar(s), s, "{s} should not be quoted");
        }
    }

    #[test]
    fn scalars_that_read_as_other_types_are_quoted() {
        for s in [
            "02134", "1", "-9", "1e5", "0x1F", "0o17", ".5", ".inf", "-.inf", ".nan", "true",
            "False", "yes", "off", "null", "~", "",
        ] {
            assert_eq!(yaml_scalar(s), format!("\"{s}\""), "{s} should be quoted");
        }
    }

    #[test]
    fn yaml_syntax_forces_quotes() {
        assert_eq!(yaml_scalar("a: b"), "\"a: b\"");
        assert_eq!(yaml_scalar("x,y"), "\"x,y\"");
        assert_eq!(yaml_scalar("[a]"), "\"[a]\"");
        assert_eq!(yaml_scalar("# note"), "\"# note\"");
        assert_eq!(yaml_scalar(" padded "), "\" padded \"");
        assert_eq!(yaml_scalar("- item"), "\"- item\"");
        assert_eq!(yaml_scalar("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(yaml_scalar("a\nb"), "\"a\\nb\"");
    }

    #[test]
    fn dates_and_timestamps_render_iso() {
        let date = render_temporal(&Value::Int(0), &ValueKind::Date);
        assert_eq!(date.as_deref(), Some("1970-01-01"));
        let date = render_temporal(&Value::Int(19_723), &ValueKind::Date);
        assert_eq!(date.as_deref(), Some("2024-01-01"));

        let kind = ValueKind::Timestamp {
            grain: TimeGrain::Micros,
            utc_adjusted: true,
        };
        let ts = render_temporal(&Value::Int(1_706_693_400_000_000), &kind);
        assert_eq!(ts.as_deref(), Some("2024-01-31T09:30:00"));
        let ts = render_temporal(&Value::Int(1_706_693_400_123_000), &kind);
        assert_eq!(ts.as_deref(), Some("2024-01-31T09:30:00.123"));
    }

    #[test]
    fn long_flow_lists_fall_back_to_block() {
        let mut out = String::new();
        let items: Vec<String> = (0..10).map(|i| format!("value_number_{i}")).collect();
        emit_list(&mut out, "        ", "examples", &items);
        assert!(out.starts_with("        examples:\n"));
        assert!(out.contains("          - value_number_0\n"));
        assert!(out.lines().all(|l| l.len() <= WIDTH));
    }

    #[test]
    fn comments_wrap_at_width() {
        let mut out = String::new();
        let text = "TODO: ".to_string() + &"word ".repeat(40);
        wrap_comment(&mut out, "    ", text.trim());
        assert!(out.lines().count() > 1);
        assert!(out.lines().all(|l| l.len() <= WIDTH));
        assert!(out.lines().all(|l| l.starts_with("    # ")));
    }

    #[test]
    fn relative_paths_walk_up_and_down() {
        let rel = relative_to(Path::new("/a/b/data/x.parquet"), Path::new("/a/b")).unwrap();
        assert_eq!(rel, "data/x.parquet");
        let rel = relative_to(Path::new("/a/c/x.parquet"), Path::new("/a/b")).unwrap();
        assert_eq!(rel, "../c/x.parquet");
        let rel = relative_to(Path::new("/a/b/x.parquet"), Path::new("/a/b")).unwrap();
        assert_eq!(rel, "x.parquet");
    }
}
