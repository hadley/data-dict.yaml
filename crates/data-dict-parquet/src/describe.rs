//! Per-column summaries of a Parquet file for `data-dict describe`.
//!
//! [`describe`] profiles a file (see [`crate::profile`]) and shapes the result
//! for presentation: a [`FileDescription`] that serializes to the `--json`
//! form directly and renders the human-readable text through [`fmt::Display`].
//! Keys that don't apply to a column's type are omitted from the JSON rather
//! than serialized as null, and scalars use their YAML-compatible forms —
//! numbers and booleans as themselves, dates and datetimes as ISO 8601
//! strings.

use std::fmt;
use std::path::Path;

use serde::Serialize;

use crate::metadata::column_type_info;
use crate::profile::{ColumnProfile, Distinct, Histogram, profile};
use crate::value::{TimeGrain, Value, ValueKind, date_iso, datetime_iso, time_iso};
use crate::{ColumnTypeInfo, ParquetError};

/// Widest bar drawn beside a histogram bin or a counted value.
const BAR_WIDTH: usize = 12;

/// Longest value label rendered in the text output before truncation.
const LABEL_WIDTH: usize = 40;

#[derive(Debug, Serialize)]
pub struct FileDescription {
    path: String,
    rows: usize,
    columns: Vec<ColumnDescription>,
}

#[derive(Debug, Serialize)]
pub struct ColumnDescription {
    name: String,
    #[serde(rename = "type")]
    dict_type: String,
    parquet_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    distinct: Option<DistinctCount>,
    /// Nulls in the column — omitted when the file's footer doesn't say.
    #[serde(skip_serializing_if = "Option::is_none")]
    missing: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<Scalar>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<Scalar>,
    #[serde(skip_serializing_if = "Option::is_none")]
    histogram: Option<HistogramView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    value_counts: Vec<ValueCountView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    examples: Vec<Scalar>,
    /// What the values mean — drives the text rendering, not the JSON.
    #[serde(skip)]
    kind: ValueKind,
}

/// The distinct-value count in its self-describing JSON form: `{"exact": n}`
/// or `{"approx": n}` — the tag is the only approximation signal.
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DistinctCount {
    Exact(usize),
    Approx(usize),
}

impl DistinctCount {
    fn count(&self) -> usize {
        match self {
            DistinctCount::Exact(count) | DistinctCount::Approx(count) => *count,
        }
    }

    fn is_approx(&self) -> bool {
        matches!(self, DistinctCount::Approx(_))
    }
}

/// A YAML-compatible scalar: numbers and booleans as themselves, temporal
/// values as ISO 8601 strings.
#[derive(Debug, Serialize, Clone)]
#[serde(untagged)]
pub enum Scalar {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

#[derive(Debug, Serialize)]
pub struct HistogramView {
    bins: Vec<BinView>,
    nan_count: usize,
    #[serde(skip_serializing_if = "is_zero")]
    negative_infinity_count: usize,
    #[serde(skip_serializing_if = "is_zero")]
    positive_infinity_count: usize,
}

#[derive(Debug, Serialize)]
pub struct BinView {
    lower: Scalar,
    upper: Scalar,
    count: usize,
    /// The pre-rendered `lower – upper` text label, sized to the histogram's
    /// bin width.
    #[serde(skip)]
    label: String,
}

#[derive(Debug, Serialize)]
pub struct ValueCountView {
    value: Scalar,
    count: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// Describe every column of the Parquet file at `path`, or just `column` when
/// given. An unknown `column` is an error that names the available columns.
pub fn describe(path: &Path, column: Option<&str>) -> Result<FileDescription, ParquetError> {
    let infos = column_type_info(path)?;
    if let Some(name) = column
        && !infos.iter().any(|info| info.name == name)
    {
        return Err(ParquetError::General(format!(
            "column `{name}` not found; available columns: {}",
            infos
                .iter()
                .map(|info| info.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let selection = column.map(|name| vec![name]);
    let file = profile(path, selection.as_deref())?;

    let columns = file
        .columns
        .into_iter()
        .map(|profile| {
            let info = infos
                .iter()
                .find(|info| info.name == profile.name)
                .expect("every profiled column comes from the schema");
            column_description(profile, info)
        })
        .collect();
    Ok(FileDescription {
        path: path.display().to_string(),
        rows: file.row_count,
        columns,
    })
}

fn column_description(profile: ColumnProfile, info: &ColumnTypeInfo) -> ColumnDescription {
    let kind = profile.kind.clone();
    let parquet_type = match &info.logical_type {
        Some(logical) => format!("{} / {logical}", info.physical_type),
        None => info.physical_type.clone(),
    };
    let distinct = profile.distinct.map(|distinct| match distinct {
        Distinct::Exact(count) => DistinctCount::Exact(count),
        Distinct::Approx(count) => DistinctCount::Approx(count),
    });
    // The extremes only mean something on a numeric scale here: text extremes
    // are lexicographic trivia, and the value list already shows booleans.
    let on_scale = kind.is_binnable();
    let scalar = |value: &Value| render_scalar(value, &kind);
    let min = on_scale.then(|| profile.min.as_ref().map(scalar)).flatten();
    let max = on_scale.then(|| profile.max.as_ref().map(scalar)).flatten();
    let histogram = profile
        .histogram
        .as_ref()
        .map(|histogram| histogram_view(histogram, &kind));
    // Frequent values are the body for text and boolean columns; a column on a
    // numeric scale summarizes its shape with the histogram instead.
    let value_counts = if matches!(kind, ValueKind::Text | ValueKind::Bool) {
        profile
            .value_counts
            .iter()
            .map(|vc| ValueCountView {
                value: render_scalar(&vc.value, &kind),
                count: vc.count,
            })
            .collect()
    } else {
        Vec::new()
    };
    ColumnDescription {
        name: profile.name,
        dict_type: info.dict_type.clone(),
        parquet_type,
        distinct,
        missing: profile.null_count,
        min,
        max,
        histogram,
        value_counts,
        examples: profile.examples.iter().map(scalar).collect(),
        kind,
    }
}

// --- scalar and label rendering ---------------------------------------------

fn render_scalar(value: &Value, kind: &ValueKind) -> Scalar {
    match (value, kind) {
        (&Value::Int(days), ValueKind::Date) => date_iso(days)
            .map(Scalar::Text)
            .unwrap_or(Scalar::Int(days)),
        (
            &Value::Int(raw),
            ValueKind::Timestamp {
                grain,
                utc_adjusted,
            },
        ) => datetime_iso(raw, *grain, *utc_adjusted)
            .map(Scalar::Text)
            .unwrap_or(Scalar::Int(raw)),
        (&Value::Int(raw), ValueKind::Time { grain }) => Scalar::Text(time_iso(raw, *grain)),
        (&Value::Int(int), _) => Scalar::Int(int),
        (&Value::Bool(bool), _) => Scalar::Bool(bool),
        (&Value::Float(float), _) => Scalar::Float(float.get()),
        (Value::Text(text), _) => Scalar::Text(text.clone()),
    }
}

fn histogram_view(histogram: &Histogram, kind: &ValueKind) -> HistogramView {
    let width = histogram
        .bins
        .first()
        .map(|bin| bin.upper - bin.lower)
        .unwrap_or(1.0);
    let bins = histogram
        .bins
        .iter()
        .map(|bin| BinView {
            lower: edge_scalar(bin.lower, kind, width),
            upper: edge_scalar(bin.upper, kind, width),
            count: bin.count,
            label: format!(
                "{} – {}",
                edge_label(bin.lower, kind, width),
                edge_label(bin.upper, kind, width)
            ),
        })
        .collect();
    HistogramView {
        bins,
        nan_count: histogram.not_finite.nan_count,
        negative_infinity_count: histogram.not_finite.negative_infinity_count,
        positive_infinity_count: histogram.not_finite.positive_infinity_count,
    }
}

/// A bin edge in the JSON: a number for numeric kinds, an ISO string for
/// temporal ones (whose raw scale — days, millis — is an implementation
/// detail no consumer should need).
fn edge_scalar(edge: f64, kind: &ValueKind, width: f64) -> Scalar {
    match kind {
        ValueKind::Date | ValueKind::Time { .. } | ValueKind::Timestamp { .. } => {
            Scalar::Text(edge_label(edge, kind, width))
        }
        _ => Scalar::Float(edge),
    }
}

/// A bin edge as text, at a precision that just distinguishes adjacent edges.
fn edge_label(edge: f64, kind: &ValueKind, width: f64) -> String {
    match kind {
        ValueKind::Date => date_iso(edge.round() as i64).unwrap_or_else(|| edge.to_string()),
        ValueKind::Timestamp { grain, .. } => {
            let per_day = grain_per_day(*grain);
            // A histogram spanning days reads best as dates; only a narrow
            // one needs the time of day.
            let datetime = datetime_iso(edge.round() as i64, *grain, false);
            match datetime {
                Some(datetime) if width >= per_day => datetime[..10].to_string(),
                Some(datetime) => datetime,
                None => edge.to_string(),
            }
        }
        ValueKind::Time { grain } => time_iso(edge.round() as i64, *grain),
        ValueKind::Int if width >= 1.0 => format!("{}", edge.round() as i64),
        _ => format!("{:.*}", decimals(width), edge),
    }
}

fn grain_per_day(grain: TimeGrain) -> f64 {
    match grain {
        TimeGrain::Millis => 86_400_000.0,
        TimeGrain::Micros => 86_400_000_000.0,
        TimeGrain::Nanos => 86_400_000_000_000.0,
    }
}

/// Decimal places that resolve steps of `width`: one digit past the width's
/// magnitude (width 2.75 → 1 decimal, width 0.275 → 2).
fn decimals(width: f64) -> usize {
    if width <= 0.0 || !width.is_finite() {
        return 1;
    }
    (1 - width.log10().floor() as i64).clamp(0, 9) as usize
}

// --- text rendering ----------------------------------------------------

impl fmt::Display for FileDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let columns = if self.columns.len() == 1 {
            "column"
        } else {
            "columns"
        };
        writeln!(
            f,
            "{}: {} rows × {} {columns}",
            self.path,
            self.rows,
            self.columns.len()
        )?;
        for column in &self.columns {
            writeln!(f)?;
            column.render(f, self.rows)?;
        }
        Ok(())
    }
}

/// One `label  bar  count` row of a histogram or value list. `suffix` trails
/// the count — the missing row carries its percentage there.
struct Row {
    label: String,
    count: usize,
    suffix: String,
}

impl Row {
    fn new(label: impl Into<String>, count: usize) -> Self {
        Row {
            label: label.into(),
            count,
            suffix: String::new(),
        }
    }
}

impl ColumnDescription {
    fn render(&self, f: &mut fmt::Formatter<'_>, rows: usize) -> fmt::Result {
        writeln!(
            f,
            "{} — {} ({})",
            self.name, self.dict_type, self.parquet_type
        )?;
        if let Some(distinct) = &self.distinct {
            let approx = if distinct.is_approx() { "~" } else { "" };
            writeln!(f, "  {:<11}{approx}{}", "distinct", distinct.count())?;
        }

        // The missing count renders once, always as the last line of the body.
        let missing = self.missing.map(|missing| {
            let mut row = Row::new("missing", missing);
            if missing > 0 {
                let percent = missing as f64 / rows.max(1) as f64 * 100.0;
                row.suffix = format!(" ({percent:.1}%)");
            }
            row
        });

        if let ValueKind::Unsupported(reason) = &self.kind {
            writeln!(f, "  not summarised ({reason})")?;
            if let Some(missing) = &missing {
                writeln!(f, "  {:<11}{}{}", "missing", missing.count, missing.suffix)?;
            }
            return Ok(());
        }

        let mut body: Vec<Row> = Vec::new();
        if let Some(histogram) = &self.histogram {
            for bin in &histogram.bins {
                body.push(Row::new(bin.label.clone(), bin.count));
            }
            if histogram.nan_count > 0 {
                body.push(Row::new("NaN", histogram.nan_count));
            }
            if histogram.negative_infinity_count > 0 {
                body.push(Row::new("-inf", histogram.negative_infinity_count));
            }
            if histogram.positive_infinity_count > 0 {
                body.push(Row::new("+inf", histogram.positive_infinity_count));
            }
        }
        for vc in &self.value_counts {
            body.push(Row::new(truncate(&scalar_label(&vc.value)), vc.count));
        }
        // The value list is capped, so say how much of the tail it hides.
        let tail = self.distinct.as_ref().and_then(|distinct| {
            let shown = self.value_counts.len();
            (shown > 0 && distinct.count() > shown).then(|| {
                let approx = if distinct.is_approx() { "~" } else { "" };
                format!("  ({approx}{} other values)", distinct.count() - shown)
            })
        });

        if body.is_empty() {
            // Nothing to bar-chart, so the missing count stands alone.
            if let Some(missing) = &missing {
                writeln!(f, "  {:<11}{}{}", "missing", missing.count, missing.suffix)?;
            }
            return Ok(());
        }
        render_rows(f, &body, tail.as_deref(), missing.as_ref())
    }
}

/// Render the body block: the bin or value `rows`, the `(n other values)`
/// tail note, then the missing row last — aligned with the rows above it.
fn render_rows(
    f: &mut fmt::Formatter<'_>,
    rows: &[Row],
    tail: Option<&str>,
    missing: Option<&Row>,
) -> fmt::Result {
    let all = || rows.iter().chain(missing);
    let max = all().map(|row| row.count).max().unwrap_or(0).max(1);
    let label_width = all()
        .map(|row| row.label.chars().count())
        .max()
        .unwrap_or(0);
    let count_width = all()
        .map(|row| row.count.to_string().len())
        .max()
        .unwrap_or(1);
    let write_row = |f: &mut fmt::Formatter<'_>, row: &Row| {
        let padding = label_width - row.label.chars().count();
        writeln!(
            f,
            "  {}{:pad$}  {:<bar$}  {:>count$}{}",
            row.label,
            "",
            bar(row.count, max),
            row.count,
            row.suffix,
            pad = padding,
            bar = BAR_WIDTH,
            count = count_width,
        )
    };
    for row in rows {
        write_row(f, row)?;
    }
    if let Some(tail) = tail {
        writeln!(f, "{tail}")?;
    }
    if let Some(missing) = missing {
        write_row(f, missing)?;
    }
    Ok(())
}

/// A bar scaled against the largest count: `▇` per block, or a sliver `▏` for
/// a count too small to earn one, so a nonzero count is never invisible.
fn bar(count: usize, max: usize) -> String {
    if count == 0 {
        return String::new();
    }
    let blocks = (count * BAR_WIDTH + max / 2) / max;
    if blocks == 0 {
        "▏".to_string()
    } else {
        "▇".repeat(blocks)
    }
}

fn scalar_label(scalar: &Scalar) -> String {
    match scalar {
        Scalar::Bool(bool) => bool.to_string(),
        Scalar::Int(int) => int.to_string(),
        Scalar::Float(float) => float.to_string(),
        Scalar::Text(text) => text.clone(),
    }
}

fn truncate(label: &str) -> String {
    if label.chars().count() <= LABEL_WIDTH {
        return label.to_string();
    }
    let mut truncated: String = label.chars().take(LABEL_WIDTH - 1).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_scale_and_never_vanish() {
        assert_eq!(bar(56, 56), "▇".repeat(12));
        assert_eq!(bar(0, 56), "");
        assert_eq!(bar(2, 56), "▏");
        assert_eq!(bar(28, 56), "▇".repeat(6));
    }

    #[test]
    fn edge_labels_match_bin_width() {
        assert_eq!(decimals(2.75), 1);
        assert_eq!(decimals(27.5), 0);
        assert_eq!(decimals(0.275), 2);
        assert_eq!(edge_label(32.1, &ValueKind::Float, 2.75), "32.1");
        assert_eq!(edge_label(3250.4, &ValueKind::Int, 152.0), "3250");
        assert_eq!(edge_label(18262.0, &ValueKind::Date, 73.0), "2020-01-01");
    }

    #[test]
    fn temporal_scalars_render_iso() {
        assert_eq!(date_iso(19_723).as_deref(), Some("2024-01-01"));
        assert_eq!(
            datetime_iso(1_706_693_400_000_000, TimeGrain::Micros, true).as_deref(),
            Some("2024-01-31T09:30:00Z")
        );
        assert_eq!(
            datetime_iso(1_706_693_400_123, TimeGrain::Millis, false).as_deref(),
            Some("2024-01-31T09:30:00.123")
        );
        assert_eq!(time_iso(34_200_000, TimeGrain::Millis), "09:30:00");
    }

    #[test]
    fn long_labels_truncate_on_char_boundaries() {
        let long = "x".repeat(60);
        let truncated = truncate(&long);
        assert_eq!(truncated.chars().count(), LABEL_WIDTH);
        assert!(truncated.ends_with('…'));
        assert_eq!(truncate("short"), "short");
    }
}
