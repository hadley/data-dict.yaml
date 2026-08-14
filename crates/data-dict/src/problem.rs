//! A single problem vocabulary for every validation level.
//!
//! A [`Problem`] is anything that can be wrong while validating — a failed spec
//! check, a column that disagrees with the data, an unreadable file. They vary
//! only on where the problem points (a YAML span / a named column / nowhere) and
//! what structured payload they carry; a code, a severity, and a message are
//! common to all. The structured payload is the flattened
//! [`ProblemKind`] tag, so a single `#[derive(Serialize)]` produces the JSON for
//! every kind uniformly.
//!
//! There is no `fatal` field by design: a problem that must stop the run is the
//! last thing a level pushes before returning, and a problem that blocks the
//! next level is caught by the driver checking [`ProblemSet::status`] before
//! descending. Fatality is control flow, not data.

use quarto_source_map::{SourceContext, SourceInfo};

use crate::Level;
use crate::report::{Failed, Report, StepKey, Steps};

/// Whether a problem blocks validation (`Error`) or is purely advisory
/// (`Warning`). Errors fail validation; warnings are reported alongside a
/// successful result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// The overall verdict for a [`ProblemSet`]: the worst severity present, or
/// `Ok` when there is nothing to report. Only `Error` fails validation; `Ok`
/// and `Warning` both pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warning,
    Error,
}

impl Status {
    /// Whether this verdict fails validation (i.e. is an error).
    pub fn failed(self) -> bool {
        self == Status::Error
    }
}

/// How to render a diagnostic to text.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderStyle {
    /// Emit ANSI colour and OSC-8 hyperlinks. Turn off for piped or redirected
    /// output; pass the destination's [`IsTerminal`](std::io::IsTerminal) state.
    pub color: bool,
    /// Replace line numbers with `LL` so snapshots don't churn when unrelated
    /// lines shift. Testing aid; leave off for real output.
    pub anonymized_line_numbers: bool,
}

/// One problem found while validating, at any level. `code`, `table` and
/// `columns` are present only when meaningful (a spec problem has a code but
/// often no table; a pre-flight failure has neither); `context` drives
/// source-highlighted rendering; `kind` is the structured payload.
///
/// The derive skips everything a JSON consumer can't use as it stands: the raw
/// `context` spans (internal byte offsets) and the `suggestion` (whose span is
/// one). [`crate::report::Report`] serializes those, resolving each span to a
/// [`SpanLocation`] through the [`SourceContext`] it holds.
#[derive(Debug, serde::Serialize)]
pub struct Problem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    /// The [`Step`](crate::report::Step) that found this problem, linked once
    /// the run is over; `None` for a problem no step accounts for (a spec
    /// problem, and `M03`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
    pub severity: Severity,
    pub message: String,
    /// The table the problem is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// Every column the problem is about, in dictionary order, each a dotted
    /// path for a struct field. Empty for a problem about no column in
    /// particular.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// What the spec expects, stated independently of this occurrence. When
    /// present it leads the rendering (the title line) and `message` reports
    /// what was found instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// Advisory text shown as a `help:` line below the excerpt. For a concrete
    /// edit, prefer [`suggestion`](Self::suggestion), which renders a patch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// A suggested fix, rendered below the excerpt as an annotate-snippets patch
    /// (a `+`/`-` diff) under a `help:` title.
    #[serde(skip)]
    pub suggestion: Option<Suggestion>,
    /// The YAML spans this problem points at, ordered outermost-first: the
    /// **last** span is the primary highlight (carrying `message`) and any
    /// preceding spans are the enclosing nodes that locate it (e.g. the table
    /// and column a bad value sits in), shown as unlabelled context lines.
    /// Empty for problems with no location (column-located and pre-flight
    /// failures). Display-only; see the type-level note on how the primary span
    /// reaches the JSON output.
    #[serde(skip)]
    pub context: Vec<SourceInfo>,
    #[serde(flatten)]
    pub kind: ProblemKind,
}

impl Problem {
    /// The primary (highlighted) span, or `None` for unlocated problems.
    fn primary_span(&self) -> Option<&SourceInfo> {
        self.context.last()
    }

    /// The enclosing context spans surrounding the primary highlight, outermost-first.
    pub(crate) fn context_spans(&self) -> &[SourceInfo] {
        self.context.split_last().map_or(&[], |(_, rest)| rest)
    }
}

/// A suggested fix: splice `replacement` into the source at `span` (an empty
/// span inserts). `title` is a lowercase description shown as the `help:` line.
#[derive(Debug, serde::Serialize)]
pub struct Suggestion {
    pub title: String,
    pub replacement: String,
    #[serde(skip)]
    pub span: SourceInfo,
}

/// One offending row's values, keyed by column name and holding only the
/// columns the check is about. Entries keep the order the check names its
/// columns in, which a map would lose. A `None` value is a missing one, and
/// serializes as JSON `null` so it stays distinct from a string column holding
/// `"null"`; only an assertion (`D07`) can report one, since the other checks
/// exempt nulls.
///
/// A restricted column is left out of the row entirely, so an entry that names
/// no columns at all means everything it would have said was withheld.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ValueRow(Vec<(String, Option<String>)>);

impl ValueRow {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Keep only the columns `keep` accepts, reporting whether any were dropped.
    pub(crate) fn retain(&mut self, keep: impl Fn(&str) -> bool) -> bool {
        let before = self.0.len();
        self.0.retain(|(column, _)| keep(column));
        self.0.len() < before
    }

    /// The values alone, in column order.
    pub fn rendered(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(_, value)| missing(value))
    }

    /// The `column=value` pairs, in column order, as an assertion reports them.
    pub fn pairs(&self) -> impl Iterator<Item = String> {
        self.0
            .iter()
            .map(|(column, value)| format!("{column}={}", missing(value)))
    }
}

/// How a missing value reads in a rendered diagnostic, matching how an
/// expression renders a null.
fn missing(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("null")
}

impl FromIterator<(String, Option<String>)> for ValueRow {
    fn from_iter<T: IntoIterator<Item = (String, Option<String>)>>(iter: T) -> Self {
        ValueRow(iter.into_iter().collect())
    }
}

impl serde::Serialize for ValueRow {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (column, value) in &self.0 {
            map.serialize_entry(column, value)?;
        }
        map.end()
    }
}

/// A resolved source span as 0-based line/column bounds, for JSON consumers.
/// Lines and columns count from 0, following the LSP convention; the
/// human-rendered diagnostics show the same positions 1-based.
#[derive(Debug, serde::Serialize)]
pub struct SpanLocation {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

/// Resolve one span to 0-based line/column bounds, or `None` when it doesn't
/// resolve in `ctx`.
pub(crate) fn span_location(span: &SourceInfo, ctx: &SourceContext) -> Option<SpanLocation> {
    let start = span.map_offset(0, ctx)?.location;
    let end = span.map_offset(span.length(), ctx)?.location;
    Some(SpanLocation {
        start_line: start.row,
        start_column: start.column,
        end_line: end.row,
        end_column: end.column,
    })
}

/// The structured payload behind a [`Problem`]. The serde tag (`"kind"`) is the
/// machine-readable discriminator; variants with fields flatten those fields
/// alongside it. Variants whose whole story is in [`Problem::message`] (the
/// pre-flight failures and the spec checks) carry no fields.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProblemKind {
    /// I/O failure reading the document.
    Io,
    /// The document is not parseable as YAML.
    Parse,
    /// The document failed structural validation against `schema.yaml`.
    Schema,
    /// The parquet file could not be read.
    Parquet,
    /// A table name was given but no such table exists.
    TableNotFound { available: Vec<String> },
    /// A semantic spec check (`S##`) failed; the specific code is in
    /// [`Problem::code`] and the detail in [`Problem::message`].
    Spec,
    /// `M01` — declared type is not compatible with the type read from the data.
    TypeMismatch { declared: String, actual: String },
    /// `M02` — column described by the dictionary but absent from the data.
    MissingInData,
    /// `M03` — column present in the data but not described by the dictionary.
    ExtraInData { actual: String },
    /// `M04` — a table validated against data declares no `source`.
    MissingSource,
    /// `M05` — a table's `source` is declared but its data can't be read (the
    /// `parquet` file is absent or unreadable).
    UnreadableSource,
    /// `D01` — a `required` (or `primary_key`) column contains nulls. `rows`
    /// lists the first few offending row numbers (1-based); `count` is the total.
    NullsInRequired {
        count: usize,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        rows: Vec<usize>,
    },
    /// `D02` — a unique column or composite primary key contains duplicates.
    /// `count` is the total; `rows` lists the first few repeat occurrences
    /// (1-based) and `values` the key they held, one entry per listed row.
    DuplicateValues {
        count: usize,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        rows: Vec<usize>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        values: Vec<ValueRow>,
        redacted: bool,
    },
    /// `D03` — a `unique` column or `primary_key` uses a type whose values can't
    /// be compared, so its uniqueness was not checked. `reason` is a short slug
    /// naming the barrier (e.g. `json`).
    UniquenessNotVerified { reason: String },
    /// `D04` — an `enum` column contains values outside its declared `values`.
    /// `count` is the total; `rows` lists the first few offending row numbers
    /// (1-based) and `values` the value each held, one entry per listed row.
    ValuesOutsideEnum {
        count: usize,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        rows: Vec<usize>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        values: Vec<ValueRow>,
        redacted: bool,
    },
    /// `D05` — a `foreign_key` column contains values absent from the
    /// `primary_key` it references. `column` is the foreign-key column,
    /// `references` names the target as `table.column`, `count` is the total,
    /// `rows` samples the first few offending row numbers (1-based) and `values`
    /// the value each held, one entry per listed row.
    ForeignKeyNotFound {
        column: String,
        references: String,
        count: usize,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        rows: Vec<usize>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        values: Vec<ValueRow>,
        redacted: bool,
    },
    /// `D06` — a `foreign_key` or the `primary_key` it references uses a type
    /// whose values can't be compared, so the reference was not checked.
    /// `references` names the target as `table.column`; `reason` is a barrier
    /// slug (e.g. `json`).
    ReferentialIntegrityNotVerified {
        column: String,
        references: String,
        reason: String,
    },
    /// `D07` — a row-level `assert` expression is false for some rows. `rows`
    /// lists the first few (1-based) and `values` the values the columns the
    /// expression reads held, one entry per listed row; `count` is the total.
    AssertionViolated {
        assertion: String,
        count: usize,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        rows: Vec<usize>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        values: Vec<ValueRow>,
        redacted: bool,
    },
    /// `D07` — an aggregate `assert` expression is false for the table. An
    /// aggregate is one verdict about every row at once, so no row is to blame.
    AssertionFalse { assertion: String },
    /// `D08` — an `assert` expression could not be evaluated against the data.
    /// `column` names the column that can't be read as its declared type, and is
    /// absent when the obstacle isn't one column's type.
    AssertionNotChecked {
        assertion: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        column: Option<String>,
        reason: String,
    },
    /// `D09` — integer arithmetic in an `assert` expression left the 64-bit
    /// range. `row` is where it happened, absent for an aggregate assertion.
    AssertionOverflow {
        assertion: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        row: Option<usize>,
    },
}

impl ProblemKind {
    /// The fixed rule code for kinds that have one. Spec checks return `None`
    /// because the code (`S01`…`S09`) varies per check and is set explicitly;
    /// pre-flight failures have no rule code.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            ProblemKind::TypeMismatch { .. } => "M01",
            ProblemKind::MissingInData => "M02",
            ProblemKind::ExtraInData { .. } => "M03",
            ProblemKind::MissingSource => "M04",
            ProblemKind::UnreadableSource => "M05",
            ProblemKind::NullsInRequired { .. } => "D01",
            ProblemKind::DuplicateValues { .. } => "D02",
            ProblemKind::UniquenessNotVerified { .. } => "D03",
            ProblemKind::ValuesOutsideEnum { .. } => "D04",
            ProblemKind::ForeignKeyNotFound { .. } => "D05",
            ProblemKind::ReferentialIntegrityNotVerified { .. } => "D06",
            ProblemKind::AssertionViolated { .. } | ProblemKind::AssertionFalse { .. } => "D07",
            ProblemKind::AssertionNotChecked { .. } => "D08",
            ProblemKind::AssertionOverflow { .. } => "D09",
            _ => return None,
        })
    }

    /// Whether the check that reported this reached a verdict at all. The
    /// kinds that didn't say why they couldn't, and leave their
    /// [`Step`](crate::report::Step) unevaluated.
    pub fn evaluated(&self) -> bool {
        !matches!(
            self,
            ProblemKind::UniquenessNotVerified { .. }
                | ProblemKind::ReferentialIntegrityNotVerified { .. }
                | ProblemKind::AssertionNotChecked { .. }
                | ProblemKind::AssertionOverflow { .. }
        )
    }

    /// The validation level this kind belongs to, when it maps to one. Pre-flight
    /// failures (`Io`/`Parse`/`Parquet`/…) return `None`.
    pub fn level(&self) -> Option<Level> {
        Some(match self {
            ProblemKind::Spec => Level::Spec,
            ProblemKind::TypeMismatch { .. }
            | ProblemKind::MissingInData
            | ProblemKind::ExtraInData { .. }
            | ProblemKind::MissingSource
            | ProblemKind::UnreadableSource => Level::Meta,
            ProblemKind::NullsInRequired { .. }
            | ProblemKind::DuplicateValues { .. }
            | ProblemKind::UniquenessNotVerified { .. }
            | ProblemKind::ValuesOutsideEnum { .. }
            | ProblemKind::ForeignKeyNotFound { .. }
            | ProblemKind::ReferentialIntegrityNotVerified { .. }
            | ProblemKind::AssertionViolated { .. }
            | ProblemKind::AssertionFalse { .. }
            | ProblemKind::AssertionNotChecked { .. }
            | ProblemKind::AssertionOverflow { .. } => Level::Data,
            _ => return None,
        })
    }
}

impl Problem {
    /// A spec-level problem (`S##`), located by a YAML span.
    pub(crate) fn spec(
        code: &'static str,
        severity: Severity,
        message: impl Into<String>,
        span: SourceInfo,
    ) -> Self {
        Problem {
            code: Some(code),
            step: None,
            severity,
            message: message.into(),
            table: None,
            columns: Vec::new(),
            expected: None,
            hint: None,
            suggestion: None,
            context: vec![span],
            kind: ProblemKind::Spec,
        }
    }

    /// `M03` — a column present in the data but not described by the
    /// dictionary. It exists only in the data, so it has no dictionary location
    /// and is named in the message rather than highlighted in source.
    pub(crate) fn undocumented_column(
        table: &str,
        name: &str,
        actual_type: impl Into<String>,
    ) -> Self {
        let actual = actual_type.into();
        Problem {
            code: Some("M03"),
            step: None,
            severity: Severity::Warning,
            message: format!("`{name}` is in the data (`{actual}`) but not the dictionary"),
            table: Some(table.to_string()),
            columns: vec![name.to_string()],
            expected: Some(
                "Every column in the data should be described in the dictionary.".into(),
            ),
            hint: None,
            suggestion: None,
            context: Vec::new(),
            kind: ProblemKind::ExtraInData { actual },
        }
    }

    /// A pre-flight failure: it carries no location, only a kind and a message.
    pub(crate) fn preflight(kind: ProblemKind, message: impl Into<String>) -> Self {
        Problem {
            code: None,
            step: None,
            severity: Severity::Error,
            message: message.into(),
            table: None,
            columns: Vec::new(),
            expected: None,
            hint: None,
            suggestion: None,
            context: Vec::new(),
            kind,
        }
    }

    /// A structural schema-validation failure, lifted from
    /// `quarto-yaml-validation` into the shared vocabulary so it renders like
    /// every other diagnostic. `expected` states the general rule (from the
    /// error's kind) and leads the rendering; `message` is the validator's
    /// concrete finding. `span` is the offending node, when the error carries
    /// one; `hint` is its fix suggestion.
    pub(crate) fn schema(
        code: &'static str,
        expected: &'static str,
        message: impl Into<String>,
        span: Option<SourceInfo>,
        hint: Option<String>,
    ) -> Self {
        Problem {
            code: Some(code),
            step: None,
            severity: Severity::Error,
            message: message.into(),
            table: None,
            columns: Vec::new(),
            expected: Some(expected.to_string()),
            hint,
            suggestion: None,
            context: span.into_iter().collect(),
            kind: ProblemKind::Schema,
        }
    }

    /// Resolve the primary span to 0-based line/column bounds (LSP convention)
    /// for JSON consumers, e.g. an editor placing the diagnostic in the file.
    /// `None` for problems with no span (column-located and pre-flight failures).
    pub fn location(&self, ctx: &SourceContext) -> Option<SpanLocation> {
        span_location(self.primary_span()?, ctx)
    }

    /// Render to display text. Span-located problems get full source
    /// highlighting; the rest render as a `severity [code]: message` line (or
    /// just the message when there is no code). When `expected` is set it leads
    /// the output and `message` follows on its own line as the "found" detail.
    pub fn to_text(&self, ctx: &SourceContext, style: RenderStyle) -> String {
        match self.primary_span() {
            Some(span) => self.render_with_source(span, ctx, style),
            None => self.render_plain(),
        }
    }

    fn render_with_source(
        &self,
        span: &SourceInfo,
        ctx: &SourceContext,
        style: RenderStyle,
    ) -> String {
        use annotate_snippets::{AnnotationKind, Group, Level, Patch, Renderer, Snippet};

        // The excerpt is drawn from the primary span's root file; drop to the
        // plain rendering if it (or its offsets) can't be resolved.
        let Some(file_id) = span.root_file_id() else {
            return self.render_plain();
        };
        let Some(file) = ctx.get_file(file_id) else {
            return self.render_plain();
        };
        let Some(content) = file.content.as_deref() else {
            return self.render_plain();
        };
        let len = content.len();
        let byte_range = |s: &SourceInfo| -> Option<std::ops::Range<usize>> {
            if s.root_file_id() != Some(file_id) {
                return None;
            }
            let start = s.map_offset(0, ctx)?.location.offset.min(len);
            let end = s
                .map_offset(s.length(), ctx)
                .map_or(start, |m| m.location.offset);
            Some(start..end.min(len).max(start))
        };
        let Some(primary) = byte_range(span) else {
            return self.render_plain();
        };

        // Enclosing nodes are shown (not folded away) but left unannotated, so
        // the location reads at a glance without underline/label clutter.
        let mut snippet = Snippet::source(content)
            .path(file.path.as_str())
            .line_start(1);
        for ctx_span in self.context_spans() {
            if let Some(range) = byte_range(ctx_span) {
                snippet = snippet.annotation(AnnotationKind::Visible.span(range));
            }
        }
        // The "found" detail sits inline beside the primary underline.
        snippet = snippet.annotation(
            AnnotationKind::Primary
                .span(primary)
                .label(self.message.as_str()),
        );

        let level = match self.severity {
            Severity::Error => Level::ERROR,
            Severity::Warning => Level::WARNING,
        };
        // The code becomes the bracketed id beside the level (`error[S07]`);
        // `expected` is the title text.
        let mut title = level.primary_title(self.expected.as_deref().unwrap_or_default());
        if let Some(code) = self.code {
            title = title.id(code);
        }
        let mut group = Group::with_title(title).element(snippet);
        if let Some(hint) = &self.hint {
            group = group.element(Level::HELP.message(hint.as_str()));
        }
        let mut groups = vec![group];

        // A suggestion becomes a secondary `help:` group whose snippet carries a
        // patch, so annotate-snippets renders it as a `+`/`-` diff.
        if let Some(suggestion) = &self.suggestion
            && let Some(range) = byte_range(&suggestion.span)
        {
            let patch = Snippet::source(content)
                .path(file.path.as_str())
                .line_start(1)
                .patch(Patch::new(range, suggestion.replacement.as_str()));
            groups.push(
                Level::HELP
                    .secondary_title(suggestion.title.as_str())
                    .element(patch),
            );
        }
        let renderer = if style.color {
            Renderer::styled()
        } else {
            Renderer::plain()
        };
        renderer
            .anonymized_line_numbers(style.anonymized_line_numbers)
            .render(&groups)
    }

    fn render_plain(&self) -> String {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let headline = self.expected.as_deref().unwrap_or(&self.message);
        let mut line = match self.code {
            Some(code) => format!("{severity} [{code}]: {headline}"),
            None => headline.to_string(),
        };
        if self.expected.is_some() {
            line.push_str(&format!("\n  {}", self.message));
        }
        if let Some(hint) = &self.hint {
            line.push_str(&format!("\nhelp: {hint}"));
        }
        if let Some(suggestion) = &self.suggestion {
            line.push_str(&format!("\nhelp: {}", suggestion.title));
        }
        line
    }
}

/// Every problem found while validating a document, with the [`SourceContext`]
/// needed to render the span-located ones. Levels push into a `ProblemSet` as
/// they run; the driver descends to the next level only while [`status`] is not
/// [`Status::Error`].
///
/// [`status`]: ProblemSet::status
#[derive(Debug)]
pub struct ProblemSet {
    pub items: Vec<Problem>,
    pub source: SourceContext,
    /// What the run checked, whether or not it found anything. Empty for a
    /// spec-level run, whose checks read the document as a whole rather than
    /// any declared target.
    pub steps: Steps,
}

impl ProblemSet {
    /// An empty set tied to a source context, ready for checks to push into.
    pub fn new(source: SourceContext) -> Self {
        ProblemSet {
            items: Vec::new(),
            source,
            steps: Steps::default(),
        }
    }

    /// Register the steps `table` implies at `level`; see [`Steps::register`].
    pub(crate) fn register_steps(
        &mut self,
        table: &crate::model::Table,
        level: Level,
        assertions: &[(String, Vec<String>)],
    ) {
        self.steps.register(table, level, assertions);
    }

    /// Record that the step `key` names found nothing.
    pub(crate) fn step_pass(&mut self, key: &StepKey) {
        self.steps.pass(key);
    }

    /// Push a problem the step `key` names found, failing that step.
    pub(crate) fn push_for(&mut self, key: &StepKey, failed: Failed, problem: Problem) {
        self.steps.fail(key, failed);
        self.push_at(key, problem);
    }

    /// Push a problem reporting that the step `key` names could not reach a
    /// verdict, leaving the step unevaluated.
    pub(crate) fn push_at(&mut self, key: &StepKey, mut problem: Problem) {
        problem.step = self.steps.id(key);
        self.push(problem);
    }

    /// Fail the step `key` names with the problem just pushed; see
    /// [`suggest_last`](Self::suggest_last) for the ordering it relies on.
    pub(crate) fn fail_last(&mut self, key: &StepKey, failed: Failed) {
        self.steps.fail(key, failed);
        let id = self.steps.id(key);
        if let Some(problem) = self.items.last_mut() {
            problem.step = id;
        }
    }

    /// Record how many rows the steps of `table` were weighed against.
    pub(crate) fn set_row_count(&mut self, table: &str, rows: usize) {
        self.steps.set_row_count(table, rows);
    }

    /// The failure that stopped the run before any check could be applied, if
    /// there was one. Such a failure is not a finding about the dictionary and
    /// has no code, so it is reported as a plain error and no report is
    /// written (see `site/report.md`).
    pub fn preflight(&self) -> Option<&Problem> {
        self.items.iter().find(|p| p.code.is_none())
    }

    /// This run's findings as the report document `site/report.md` specifies.
    pub fn report(&self) -> Report<'_> {
        Report::new(self)
    }

    /// A set holding a single pre-flight failure, with no source. Used when
    /// validation could not begin (unreadable or unparseable document).
    pub fn from_preflight(kind: ProblemKind, message: impl Into<String>) -> Self {
        let mut set = ProblemSet::new(SourceContext::new());
        set.push(Problem::preflight(kind, message));
        set
    }

    pub fn push(&mut self, problem: Problem) {
        self.items.push(problem);
    }

    /// Push a problem located in the document: `expected` states the rule,
    /// `actual` reports what was found, and `spans` locates it — the **last**
    /// span is the primary highlight carrying `actual`, and any preceding spans
    /// are shown as enclosing context (outermost-first, e.g. the table then the
    /// column), each with a role label. `spans` must be non-empty.
    fn push_located_problem(
        &mut self,
        code: &'static str,
        kind: ProblemKind,
        severity: Severity,
        expected: impl Into<String>,
        actual: impl Into<String>,
        spans: impl IntoIterator<Item = SourceInfo>,
    ) {
        let spans: Vec<SourceInfo> = spans.into_iter().collect();
        assert!(
            !spans.is_empty(),
            "a located problem needs at least the primary span"
        );
        self.push(Problem {
            code: Some(code),
            step: None,
            severity,
            message: actual.into(),
            table: None,
            columns: Vec::new(),
            expected: Some(expected.into()),
            hint: None,
            suggestion: None,
            context: spans,
            kind,
        });
    }

    /// Run `f`, saying that everything it reports is about `table` and
    /// `columns` unless the check itself said something more precise. Lets the
    /// spec checks name what they are about without every one of them passing
    /// it along.
    pub(crate) fn scope<R>(
        &mut self,
        table: &str,
        columns: &[String],
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let from = self.items.len();
        let result = f(self);
        for problem in &mut self.items[from..] {
            problem.table.get_or_insert_with(|| table.to_string());
            if problem.columns.is_empty() {
                problem.columns = columns.to_vec();
            }
        }
        result
    }

    /// Say what the most recently pushed problem is about: the table, and the
    /// columns within it (a dotted path for a struct field). See
    /// [`suggest_last`](Self::suggest_last) for the ordering it relies on.
    pub(crate) fn at_last(
        &mut self,
        table: impl Into<String>,
        columns: impl IntoIterator<Item = String>,
    ) {
        if let Some(problem) = self.items.last_mut() {
            problem.table = Some(table.into());
            problem.columns = columns.into_iter().collect();
        }
    }

    /// Attach a fix suggestion to the most recently pushed problem. Called right
    /// after a `push_*` so `items.last` is that problem (the sort into source
    /// order happens once, after every check has run).
    pub(crate) fn suggest_last(&mut self, suggestion: Suggestion) {
        if let Some(problem) = self.items.last_mut() {
            problem.suggestion = Some(suggestion);
        }
    }

    /// Attach a fix hint to the most recently pushed problem; see
    /// [`suggest_last`](Self::suggest_last) for the ordering it relies on.
    pub(crate) fn hint_last(&mut self, hint: impl Into<String>) {
        if let Some(problem) = self.items.last_mut() {
            problem.hint = Some(hint.into());
        }
    }

    /// Push a spec problem (`S##`) at error severity; see
    /// [`push_located_problem`](Self::push_located_problem) for the `spans`
    /// convention. A spec check with no rule statement (a bare parse failure)
    /// builds its [`Problem`] directly.
    pub(crate) fn push_spec_error(
        &mut self,
        code: &'static str,
        expected: impl Into<String>,
        actual: impl Into<String>,
        spans: impl IntoIterator<Item = SourceInfo>,
    ) {
        self.push_located_problem(
            code,
            ProblemKind::Spec,
            Severity::Error,
            expected,
            actual,
            spans,
        );
    }

    /// Push a spec problem at warning severity; see [`push_spec_error`](Self::push_spec_error).
    pub(crate) fn push_spec_warning(
        &mut self,
        code: &'static str,
        expected: impl Into<String>,
        actual: impl Into<String>,
        spans: impl IntoIterator<Item = SourceInfo>,
    ) {
        self.push_located_problem(
            code,
            ProblemKind::Spec,
            Severity::Warning,
            expected,
            actual,
            spans,
        );
    }

    /// Push a metadata/data problem located at the dictionary node it concerns;
    /// `code` and any structured payload come from `kind`. See
    /// [`push_located_problem`](Self::push_located_problem) for the `spans`
    /// convention.
    pub(crate) fn push_located(
        &mut self,
        kind: ProblemKind,
        severity: Severity,
        expected: impl Into<String>,
        actual: impl Into<String>,
        spans: impl IntoIterator<Item = SourceInfo>,
    ) {
        let code = kind
            .code()
            .expect("a located metadata/data kind has a code");
        self.push_located_problem(code, kind, severity, expected, actual, spans);
    }

    /// Order span-located problems by their position in the document. Checks
    /// emit in their own order, but readers expect source order; the sort is
    /// stable, and problems without a resolvable span (column/pre-flight) sort
    /// last.
    pub fn sort(&mut self) {
        self.items.sort_by_key(|p| {
            p.primary_span()
                .and_then(|s| s.resolve_byte_range())
                .map(|(file, start, _)| (file, start))
                .unwrap_or((usize::MAX, usize::MAX))
        });
    }

    /// The overall verdict: [`Status::Error`] if anything is an error,
    /// [`Status::Warning`] if there are only warnings, [`Status::Ok`] if there
    /// is nothing to report.
    pub fn status(&self) -> Status {
        let mut status = Status::Ok;
        for p in &self.items {
            match p.severity {
                Severity::Error => return Status::Error,
                Severity::Warning => status = Status::Warning,
            }
        }
        status
    }

    /// Render every problem to display text, in their current order. See
    /// [`RenderStyle`] for the colour and line-number options.
    pub fn render(&self, style: RenderStyle) -> Vec<String> {
        self.items
            .iter()
            .map(|p| p.to_text(&self.source, style))
            .collect()
    }
}

/// Build a sub-span pointing at `[start, end)` byte offsets within `parent`.
/// Returns `None` if `parent` does not resolve to a single contiguous byte
/// range (Concat / FilterProvenance variants, which YAML scalar literals
/// shouldn't produce in practice).
pub(crate) fn subspan(parent: &SourceInfo, start: usize, end: usize) -> Option<SourceInfo> {
    parent.resolve_byte_range()?;
    Some(SourceInfo::substring(parent.clone(), start, end))
}

/// Format offending row numbers for display: `rows: 3, 7, 12`, with a trailing
/// `, …` when there were more offenders than the recorded sample. The label is
/// singular (`row: 3`) for a lone offender.
pub(crate) fn format_rows(rows: &[usize], count: usize) -> String {
    let label = if count == 1 { "row" } else { "rows" };
    let listed = rows
        .iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if count > rows.len() {
        format!("{label}: {listed}, …")
    } else {
        format!("{label}: {listed}")
    }
}

/// Format offending values for display: `` `sleepy`, `grumpy` ``, or
/// `` `(1, 2020-01-01)` `` once a row names more than one column. Repeats are
/// dropped, since a reader learns nothing from seeing the same value twice —
/// unlike the rows, which line up with [`ValueRow`]s one for one. `None` when
/// every value was withheld as restricted.
pub(crate) fn format_values(values: &[ValueRow]) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    for row in values {
        if row.is_empty() {
            continue;
        }
        let listed = row.rendered().collect::<Vec<_>>();
        let text = match listed.as_slice() {
            [only] => format!("`{only}`"),
            many => format!("`({})`", many.join(", ")),
        };
        if !seen.contains(&text) {
            seen.push(text);
        }
    }
    (!seen.is_empty()).then(|| seen.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undocumented_column_json_flattens_kind_with_code_and_columns() {
        let p = Problem::undocumented_column("otters", "notes", "string");
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({
                "code": "M03",
                "severity": "warning",
                "message": "`notes` is in the data (`string`) but not the dictionary",
                "expected": "Every column in the data should be described in the dictionary.",
                "table": "otters",
                "columns": ["notes"],
                "kind": "extra_in_data",
                "actual": "string",
            })
        );
    }

    #[test]
    fn spec_problem_json_carries_code_and_message_no_column() {
        let p = Problem::spec("S07", Severity::Error, "bad column", SourceInfo::for_test());
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["code"], "S07");
        assert_eq!(v["kind"], "spec");
        assert_eq!(v["message"], "bad column");
        assert!(v.get("columns").is_none());
    }

    #[test]
    fn plain_problem_renders_expected_then_found() {
        let p = Problem::undocumented_column("otters", "notes", "string");
        assert_eq!(
            p.to_text(&SourceContext::new(), RenderStyle::default()),
            "warning [M03]: Every column in the data should be described in the dictionary.\n  \
             `notes` is in the data (`string`) but not the dictionary",
        );
    }

    #[test]
    fn preflight_problem_json_has_kind_but_no_code() {
        let p = Problem::preflight(
            ProblemKind::TableNotFound {
                available: vec!["a".into(), "b".into()],
            },
            "table \"x\" is not in the data dictionary",
        );
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "table_not_found");
        assert_eq!(v["available"], serde_json::json!(["a", "b"]));
        assert_eq!(v["severity"], "error");
        assert!(v.get("code").is_none());
    }

    #[test]
    fn codes_and_levels() {
        assert_eq!(ProblemKind::MissingInData.code(), Some("M02"));
        assert_eq!(ProblemKind::MissingInData.level(), Some(Level::Meta));
        assert_eq!(
            ProblemKind::NullsInRequired {
                count: 1,
                rows: vec![2]
            }
            .level(),
            Some(Level::Data)
        );
        assert_eq!(
            ProblemKind::DuplicateValues {
                count: 1,
                rows: vec![2],
                values: vec![row(&[("id", Some("x"))])],
                redacted: false,
            }
            .code(),
            Some("D02")
        );
        assert_eq!(
            ProblemKind::ValuesOutsideEnum {
                count: 1,
                rows: vec![2],
                values: vec![row(&[("status", Some("x"))])],
                redacted: false,
            }
            .code(),
            Some("D04")
        );
        assert_eq!(ProblemKind::Io.code(), None);
        assert_eq!(ProblemKind::Io.level(), None);
    }

    #[test]
    fn row_formatting() {
        assert_eq!(format_rows(&[2], 1), "row: 2");
        assert_eq!(format_rows(&[2, 5, 9], 3), "rows: 2, 5, 9");
        assert_eq!(format_rows(&[1, 2, 3, 4, 5], 8), "rows: 1, 2, 3, 4, 5, …");
    }

    fn row(pairs: &[(&str, Option<&str>)]) -> ValueRow {
        pairs
            .iter()
            .map(|(column, value)| ((*column).to_string(), value.map(str::to_string)))
            .collect()
    }

    #[test]
    fn value_formatting() {
        assert_eq!(
            format_values(&[row(&[("status", Some("sleepy"))])]).as_deref(),
            Some("`sleepy`")
        );
        assert_eq!(
            format_values(&[row(&[("a", Some("1")), ("b", Some("2020-01-01"))])]).as_deref(),
            Some("`(1, 2020-01-01)`")
        );
        assert_eq!(
            format_values(&[
                row(&[("status", Some("sleepy"))]),
                row(&[("status", Some("sleepy"))]),
                row(&[("status", Some("grumpy"))]),
            ])
            .as_deref(),
            Some("`sleepy`, `grumpy`")
        );
        assert_eq!(
            format_values(&[row(&[("weight", None)])]).as_deref(),
            Some("`null`")
        );
        assert_eq!(format_values(&[ValueRow::default()]), None);
        assert_eq!(format_values(&[]), None);
    }

    #[test]
    fn value_row_serializes_as_an_object_in_column_order() {
        let values = vec![row(&[("b", Some("2")), ("a", Some("1")), ("c", None)])];
        assert_eq!(
            serde_json::to_string(&values).unwrap(),
            r#"[{"b":"2","a":"1","c":null}]"#
        );
    }
}
