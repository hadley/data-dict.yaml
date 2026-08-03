//! Per-column profiles of a Parquet file's data.
//!
//! A profile answers "what is in this column?" — how many rows and nulls, how
//! many distinct values, the extremes, the frequent values, the shape of the
//! distribution, and a few examples. It is the shared engine behind the
//! `describe` and `draft` commands, which are two views over the same summary.
//!
//! Columns are profiled in one streaming pass each, and in parallel with one
//! another. Every summary is bounded in size, so a column of a billion distinct
//! values costs the same memory as one with ten (see [`crate::sketch`]); when a
//! bound is reached the affected numbers are marked approximate rather than
//! silently wrong.

use std::path::Path;

use arrow_array::cast::AsArray;
use arrow_array::types::Int32Type;
use arrow_array::{Array, ArrayRef};
use arrow_schema::DataType;
use parquet::basic::PageType;
use parquet::file::metadata::{ColumnChunkMetaData, ParquetMetaData};
use parquet::file::statistics::Statistics;
use parquet::schema::types::{SchemaDescriptor, Type};
use rayon::prelude::*;

use crate::ParquetError;
use crate::page::is_dictionary;
use crate::reader::FileContext;
use crate::sketch::{BottomK, HyperLogLog, SpaceSaving, ValueCount, hash_value};
use crate::value::{Decoded, Repr, Value, ValueColumn, ValueKind, classify};

/// Distinct values counted exactly per column before counts turn approximate.
const TRACKED_VALUES: usize = 1_000;
/// Distinct values sampled per column to draw examples from.
const SAMPLED_VALUES: usize = 128;
/// Most frequent values reported.
const TOP_VALUES: usize = 20;
/// Examples reported.
const EXAMPLES: usize = 5;
/// Histogram bins spanning a column's range.
const BINS: usize = 20;

/// A summary of one column's data.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnProfile {
    pub name: String,
    pub kind: ValueKind,
    /// Nulls in the column. For a [`ValueKind::Unsupported`] column, which is
    /// never read, this is the footer's count and falls back to 0 when the file
    /// carries no statistics.
    pub null_count: usize,
    /// How many distinct values the column holds. `None` for a column whose
    /// values were never counted: one that was never read, or a continuous
    /// kind, where floating-point noise makes per-value equality misleading
    /// (see [`ValueKind::is_continuous`]) — its shape is the histogram.
    pub distinct: Option<Distinct>,
    /// The extremes, for ordered kinds with at least one value.
    pub min: Option<Value>,
    pub max: Option<Value>,
    /// The [`TOP_VALUES`] most frequent values, most frequent first. Empty for
    /// continuous kinds, which aren't counted per value.
    pub value_counts: Vec<ValueCount>,
    /// The distribution across [`BINS`] equal-width bins, for kinds on a
    /// numeric scale that hold at least one value.
    pub histogram: Option<Histogram>,
    /// Up to [`EXAMPLES`] representative values, spread along the sorted
    /// distinct values.
    pub examples: Vec<Value>,
}

/// A summary of one Parquet file's data.
#[derive(Debug, Clone, PartialEq)]
pub struct FileProfile {
    pub row_count: usize,
    pub columns: Vec<ColumnProfile>,
}

/// A distinct-value count, which stops being exact once a column exceeds
/// [`TRACKED_VALUES`] distinct values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distinct {
    Exact(usize),
    Approx(usize),
}

impl Distinct {
    pub fn count(self) -> usize {
        match self {
            Distinct::Exact(count) | Distinct::Approx(count) => count,
        }
    }
}

/// How a column's values are distributed, plus the float values that have no
/// place in that distribution.
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    /// Equal-width bins spanning the column's finite range, low to high. Nulls
    /// are never binned — that is `null_count`, which applies to every type.
    pub bins: Vec<Bin>,
    pub not_finite: NotFinite,
}

/// Float values with no place on the number line, tallied by what they are.
///
/// These reach none of the rest of a profile — not the bins, the extremes, the
/// distinct count or the examples. A NaN isn't equal to itself, and an infinity
/// as a minimum or a maximum would stretch every bin to infinite width, so
/// [`F64::new`] refuses to make a value of either. Counting them keeps them
/// visible without letting them distort everything else. All zero for any
/// column that isn't a float.
///
/// [`F64::new`]: crate::F64::new
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NotFinite {
    pub nan_count: usize,
    pub negative_infinity_count: usize,
    pub positive_infinity_count: usize,
}

impl NotFinite {
    fn add(&mut self, value: f64, count: usize) {
        if value.is_nan() {
            self.nan_count += count;
        } else if value.is_sign_negative() {
            self.negative_infinity_count += count;
        } else {
            self.positive_infinity_count += count;
        }
    }

    /// Whether anything at all was counted, so a column of nothing but these
    /// still has something to report.
    pub fn any(self) -> bool {
        self.nan_count + self.negative_infinity_count + self.positive_infinity_count > 0
    }
}

/// Counts the values in `(lower, upper]`, or `[lower, upper]` for the first bin
/// so that the minimum has a home. The rule is carried on the bin rather than
/// left as a convention, so every consumer sees the boundary explicitly.
///
/// Bounds are `f64` because equal-width bins over an integer or temporal range
/// generally fall between representable values; a column's [`ValueKind`] gives
/// the unit they are measured in.
#[derive(Debug, Clone, PartialEq)]
pub struct Bin {
    pub lower: f64,
    pub upper: f64,
    pub lower_inclusive: bool,
    pub count: usize,
}

/// Profile every column of a Parquet file, or just `columns` when given, in the
/// order requested. Unknown column names are an error.
pub fn profile(path: &Path, columns: Option<&[&str]>) -> Result<FileProfile, ParquetError> {
    let ctx = FileContext::open(path)?;
    let meta = ctx.parquet();
    let row_count = ctx.rows();
    let targets = select(meta, columns)?;

    // Columns are independent, and each holds only its own bounded summary
    // while it runs, so the peak memory is set by the pool size rather than by
    // how wide the file is.
    let columns = targets
        .par_iter()
        .map(|target| {
            if target.readable() {
                profile_column(path, target)
            } else {
                Ok(stub(target, footer_nulls(meta, target.leaf)))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(FileProfile { row_count, columns })
}

/// A column to profile: what it holds, and where to read it.
struct Target {
    name: String,
    kind: ValueKind,
    repr: Repr,
    /// The column's leaf position, absent when it isn't a readable primitive.
    leaf: Option<usize>,
}

impl Target {
    /// Whether the column's values can be read and summarized.
    fn readable(&self) -> bool {
        self.leaf.is_some() && !matches!(self.repr, Repr::Unsupported)
    }
}

fn select(meta: &ParquetMetaData, columns: Option<&[&str]>) -> Result<Vec<Target>, ParquetError> {
    let fields = meta.file_metadata().schema().get_fields();
    let descr = meta.file_metadata().schema_descr();
    let target = |field: &Type| {
        let (kind, repr) = classify(field);
        Target {
            name: field.name().to_string(),
            kind,
            repr,
            leaf: find_leaf(descr, field.name()),
        }
    };
    let Some(names) = columns else {
        return Ok(fields.iter().map(|field| target(field)).collect());
    };
    names
        .iter()
        .map(|name| {
            fields
                .iter()
                .find(|field| field.name() == *name)
                .map(|field| target(field))
                .ok_or_else(|| ParquetError::General(format!("Column not found: {name}")))
        })
        .collect()
}

/// The leaf index of the column named `name`, if it is a readable primitive.
fn find_leaf(descr: &SchemaDescriptor, name: &str) -> Option<usize> {
    (0..descr.num_columns()).find(|&i| descr.column(i).name() == name)
}

/// The profile of a column whose values are never read.
fn stub(target: &Target, null_count: usize) -> ColumnProfile {
    ColumnProfile {
        name: target.name.clone(),
        kind: target.kind.clone(),
        null_count,
        distinct: None,
        min: None,
        max: None,
        value_counts: Vec::new(),
        histogram: None,
        examples: Vec::new(),
    }
}

fn footer_nulls(meta: &ParquetMetaData, leaf: Option<usize>) -> usize {
    let Some(leaf) = leaf else {
        return 0;
    };
    meta.row_groups()
        .iter()
        .try_fold(0usize, |total, group| {
            group
                .column(leaf)
                .statistics()
                .and_then(|statistics| statistics.null_count_opt())
                .map(|nulls| total + nulls as usize)
        })
        .unwrap_or(0)
}

fn profile_column(path: &Path, target: &Target) -> Result<ColumnProfile, ParquetError> {
    let ctx = FileContext::open(path)?;
    let leaf = ctx
        .leaf(&target.name)
        .ok_or_else(|| ParquetError::General(format!("Column not found: {}", target.name)))?;

    // Taking the range from the footer keeps the bin edges known before the
    // pass, so values can be binned as they stream by.
    let range = target
        .kind
        .is_binnable()
        .then(|| footer_range(ctx.parquet(), leaf, target.repr))
        .flatten();
    let mut accumulator = Accumulator::new(target, range);
    scan(&ctx, leaf, &mut accumulator)?;

    if accumulator.unprofilable {
        let target = Target {
            name: target.name.clone(),
            kind: ValueKind::Unsupported("non-UTF-8"),
            repr: Repr::Unsupported,
            leaf: target.leaf,
        };
        // The scan stopped where the bad value was, so its running null count
        // covers only part of the column; the footer's covers all of it.
        let nulls = footer_nulls(ctx.parquet(), target.leaf);
        return Ok(stub(&target, nulls));
    }

    // Without footer statistics the range is only known once the values have
    // been seen, so binning them takes a second pass. Nearly every writer
    // records min/max, making this the rare path — but a file that lacks them
    // is still worth describing, so it costs a re-read rather than a missing
    // histogram. Nothing is buffered in between: the first pass already
    // reduced the column to its extremes.
    if accumulator.bins.is_none()
        && target.kind.is_binnable()
        && let Some(bins) = BinCounts::spanning(&accumulator.min, &accumulator.max)
    {
        let mut binner = BinOnly { bins };
        scan(&ctx, leaf, &mut binner)?;
        accumulator.bins = Some(binner.bins);
    }
    Ok(accumulator.finish(target))
}

/// The `[min, max]` range every row group's footer agrees on, or `None` if any
/// of them can't supply an exact one.
fn footer_range(meta: &ParquetMetaData, leaf: usize, repr: Repr) -> Option<(f64, f64)> {
    // An unsigned column's footer extremes are ordered by the unsigned reading
    // of bits a raw page stores signed, so they can't be compared as read.
    if matches!(repr, Repr::Uint) {
        return None;
    }
    let mut range: Option<(f64, f64)> = None;
    for group in meta.row_groups() {
        let column = group.column(leaf);
        if column.num_values() == 0 {
            continue;
        }
        let statistics = column.statistics()?;
        if !statistics.min_is_exact() || !statistics.max_is_exact() {
            return None;
        }
        let (low, high) = match statistics {
            Statistics::Int32(values) => (*values.min_opt()? as f64, *values.max_opt()? as f64),
            Statistics::Int64(values) => (*values.min_opt()? as f64, *values.max_opt()? as f64),
            Statistics::Float(values) => (*values.min_opt()? as f64, *values.max_opt()? as f64),
            Statistics::Double(values) => (*values.min_opt()?, *values.max_opt()?),
            _ => return None,
        };
        // A column of nothing but NaN reports one as its range in some writers,
        // and an infinity gives bins no width to divide.
        if !low.is_finite() || !high.is_finite() || low > high {
            return None;
        }
        range = Some(match range {
            Some((seen_low, seen_high)) => (seen_low.min(low), seen_high.max(high)),
            None => (low, high),
        });
    }
    range
}

/// A destination for the values a scan produces. Implemented twice: once to
/// build a whole profile, and once to only fill in a histogram.
trait Observe {
    fn observe(&mut self, decoded: Decoded, count: usize);

    fn observe_null(&mut self, count: usize);

    /// Whether the column turned out not to be profilable after all, so there
    /// is nothing to gain by reading the rest of it.
    fn abandoned(&self) -> bool {
        false
    }
}

/// Everything a profile needs, accumulated in bounded space.
struct Accumulator {
    /// Whether the values are ordered, so tracking extremes is meaningful.
    ordered: bool,
    /// Whether the values are a continuous measure, whose per-value equality
    /// is not meaningful — no distinct count or frequent values.
    continuous: bool,
    nulls: usize,
    not_finite: NotFinite,
    min: Option<Value>,
    max: Option<Value>,
    counts: SpaceSaving,
    distinct: HyperLogLog,
    sample: BottomK,
    bins: Option<BinCounts>,
    unprofilable: bool,
}

impl Accumulator {
    fn new(target: &Target, range: Option<(f64, f64)>) -> Self {
        Accumulator {
            ordered: target.kind.is_ordered(),
            continuous: target.kind.is_continuous(),
            nulls: 0,
            not_finite: NotFinite::default(),
            min: None,
            max: None,
            counts: SpaceSaving::new(TRACKED_VALUES),
            distinct: HyperLogLog::new(),
            sample: BottomK::new(SAMPLED_VALUES),
            bins: range.map(|(low, high)| BinCounts::new(low, high)),
            unprofilable: false,
        }
    }

    fn finish(self, target: &Target) -> ColumnProfile {
        let distinct = if self.continuous {
            None
        } else if self.counts.is_saturated() {
            Some(Distinct::Approx(self.distinct.estimate()))
        } else {
            Some(Distinct::Exact(self.counts.len()))
        };
        let histogram = match self.bins {
            Some(bins) => Some(bins.finish(self.not_finite)),
            // No value had a place on the number line, so there is no range to
            // bin — but what was there is still worth reporting.
            None if self.not_finite.any() && target.kind.is_binnable() => Some(Histogram {
                bins: Vec::new(),
                not_finite: self.not_finite,
            }),
            None => None,
        };
        ColumnProfile {
            name: target.name.clone(),
            kind: target.kind.clone(),
            null_count: self.nulls,
            distinct,
            min: self.min,
            max: self.max,
            value_counts: self.counts.top(TOP_VALUES),
            histogram,
            examples: self.sample.examples(EXAMPLES),
        }
    }
}

impl Observe for Accumulator {
    fn observe(&mut self, decoded: Decoded, count: usize) {
        let value = match decoded {
            Decoded::Value(value) => value,
            Decoded::NotFinite(float) => {
                self.not_finite.add(float, count);
                return;
            }
            Decoded::NotUtf8 => {
                self.unprofilable = true;
                return;
            }
        };
        // A sample of unique values is kept for every kind — for a continuous
        // column the bitwise dedup is only there to spread the sample, so the
        // split it makes (of, say, the signed zeros) is harmless.
        let hash = hash_value(&value);
        self.sample.insert(hash, &value);
        if !self.continuous {
            self.distinct.insert(hash);
            self.counts.add(&value, count);
        }
        if let (Some(bins), Some(number)) = (self.bins.as_mut(), value.as_f64()) {
            bins.add(number, count);
        }
        if self.ordered {
            if self.min.as_ref().is_none_or(|min| value < *min) {
                self.min = Some(value.clone());
            }
            if self.max.as_ref().is_none_or(|max| value > *max) {
                self.max = Some(value);
            }
        }
    }

    fn observe_null(&mut self, count: usize) {
        self.nulls += count;
    }

    fn abandoned(&self) -> bool {
        self.unprofilable
    }
}

/// Fills in a histogram on a second pass, once the first has established the
/// range the footer didn't provide.
struct BinOnly {
    bins: BinCounts,
}

impl Observe for BinOnly {
    fn observe(&mut self, decoded: Decoded, count: usize) {
        if let Decoded::Value(value) = decoded
            && let Some(number) = value.as_f64()
        {
            self.bins.add(number, count);
        }
    }

    fn observe_null(&mut self, _count: usize) {}
}

/// Counts falling in each of [`BINS`] equal-width bins spanning `[low, high]`.
struct BinCounts {
    low: f64,
    high: f64,
    counts: Vec<usize>,
}

impl BinCounts {
    fn new(low: f64, high: f64) -> Self {
        // A single value spans no range, so it gets one bin of its own rather
        // than twenty empty ones around it.
        let bins = if low < high { BINS } else { 1 };
        BinCounts {
            low,
            high,
            counts: vec![0; bins],
        }
    }

    /// The bins spanning an observed range, or `None` when there was nothing to
    /// bin or the values aren't numbers. The extremes are finite by
    /// construction (see [`crate::F64::new`]); the check restates that here, so
    /// that a non-finite edge can never silently turn every bin into NaN.
    fn spanning(min: &Option<Value>, max: &Option<Value>) -> Option<BinCounts> {
        let low = min.as_ref()?.as_f64()?;
        let high = max.as_ref()?.as_f64()?;
        (low.is_finite() && high.is_finite()).then(|| BinCounts::new(low, high))
    }

    fn width(&self) -> f64 {
        (self.high - self.low) / self.counts.len() as f64
    }

    fn add(&mut self, value: f64, count: usize) {
        let last = self.counts.len() - 1;
        // Bins are open below, so a value sits in the bin its distance from the
        // minimum rounds up into; only the minimum itself belongs to the first.
        let index = if value <= self.low {
            0
        } else {
            (((value - self.low) / self.width()).ceil() as usize).saturating_sub(1)
        };
        self.counts[index.min(last)] += count;
    }

    fn finish(self, not_finite: NotFinite) -> Histogram {
        let width = self.width();
        let bins = self
            .counts
            .iter()
            .enumerate()
            .map(|(index, &count)| Bin {
                lower: self.low + width * index as f64,
                upper: self.low + width * (index + 1) as f64,
                lower_inclusive: index == 0,
                count,
            })
            .collect();
        Histogram { bins, not_finite }
    }
}

/// Stream one column through `observer`, one row group at a time.
///
/// A chunk whose footer says it is dictionary-encoded is read as a
/// `Dictionary(Int32, _)` array: its distinct values decode once and its rows
/// arrive as indices, so a million rows over ten values cost ten observations
/// per batch rather than a million. Any other chunk decodes plainly. The
/// choice is only a performance heuristic — both readings are correct, and a
/// mixed-encoding chunk read as a dictionary is hydrated by arrow.
fn scan(ctx: &FileContext, leaf: usize, observer: &mut impl Observe) -> Result<(), ParquetError> {
    for group in 0..ctx.parquet().num_row_groups() {
        let chunk = ctx.parquet().row_group(group).column(leaf);
        if chunk.num_values() == 0 {
            continue;
        }
        scan_group(ctx, leaf, group, prefer_dictionary(chunk), observer)?;
        if observer.abandoned() {
            return Ok(());
        }
    }
    Ok(())
}

/// Decode one row group's values into `observer`.
fn scan_group(
    ctx: &FileContext,
    leaf: usize,
    group: usize,
    dictionary: bool,
    observer: &mut impl Observe,
) -> Result<(), ParquetError> {
    let reader = if dictionary {
        ctx.dictionary_group_reader(leaf, group)?
    } else {
        ctx.group_reader([leaf], group)?
    };
    for batch in reader {
        let batch = batch?;
        let array = batch.column(0);
        match array.data_type() {
            DataType::Dictionary(_, _) => observe_dictionary(array, observer)?,
            _ => observe_plain(array, observer)?,
        }
        if observer.abandoned() {
            return Ok(());
        }
    }
    Ok(())
}

/// Observe every row of a plainly-decoded batch, one value at a time.
fn observe_plain(array: &ArrayRef, observer: &mut impl Observe) -> Result<(), ParquetError> {
    let values = ValueColumn::new(array)?;
    let validity = array.nulls();
    for row in 0..array.len() {
        if validity.is_some_and(|v| v.is_null(row)) {
            observer.observe_null(1);
        } else {
            observer.observe(values.get(row), 1);
        }
    }
    Ok(())
}

/// Observe a dictionary-decoded batch by tallying its indices, so each distinct
/// value in the batch is observed once with its total count.
fn observe_dictionary(array: &ArrayRef, observer: &mut impl Observe) -> Result<(), ParquetError> {
    let dictionary = array.as_dictionary::<Int32Type>();
    let values = ValueColumn::new(dictionary.values())?;
    let keys = dictionary.keys();
    let mut counts = vec![0usize; dictionary.values().len()];
    let mut nulls = 0usize;
    let validity = keys.nulls();
    for row in 0..keys.len() {
        if validity.is_some_and(|v| v.is_null(row)) {
            nulls += 1;
        } else {
            counts[keys.value(row) as usize] += 1;
        }
    }
    observer.observe_null(nulls);
    for (index, count) in counts.into_iter().enumerate() {
        if count > 0 {
            observer.observe(values.get(index), count);
            if observer.abandoned() {
                break;
            }
        }
    }
    Ok(())
}

/// Whether the footer rules out a non-dictionary data page. Absent stats aren't
/// a rejection: each page's own encoding is checked as it is read.
fn encoding_stats_all_dictionary(chunk: &ColumnChunkMetaData) -> bool {
    let Some(stats) = chunk.page_encoding_stats() else {
        return true;
    };
    stats
        .iter()
        .filter(|stat| matches!(stat.page_type, PageType::DATA_PAGE | PageType::DATA_PAGE_V2))
        .all(|stat| is_dictionary(stat.encoding))
}

/// A writer abandons dictionary encoding mid-chunk once the dictionary page
/// hits its size limit (1 MB by default), and the footer doesn't always say so
/// (`page_encoding_stats` are optional). A dictionary page near that limit is
/// the fallback's fingerprint, so such a chunk is read plainly rather than
/// paying arrow to hydrate its plain pages into per-batch dictionaries.
const LIKELY_FALLBACK_DICTIONARY_BYTES: i64 = 512 * 1024;

/// Whether to read a chunk as a dictionary array: its distinct values decode
/// once and its rows arrive as indices. Wrong guesses stay correct — arrow
/// hydrates or unwraps as needed — they just cost time.
fn prefer_dictionary(chunk: &ColumnChunkMetaData) -> bool {
    let Some(dictionary_offset) = chunk.dictionary_page_offset() else {
        return false;
    };
    if !encoding_stats_all_dictionary(chunk) {
        return false;
    }
    chunk.data_page_offset() - dictionary_offset < LIKELY_FALLBACK_DICTIONARY_BYTES
}

#[cfg(test)]
mod tests {
    use super::{BINS, BinCounts, NotFinite, find_leaf, scan_group};
    use crate::reader::FileContext;
    use crate::value::Value;
    use parquet::file::properties::{WriterProperties, WriterVersion};
    use parquet::file::writer::SerializedFileWriter;
    use parquet::schema::parser::parse_message_type;
    use std::fs::File;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Collects what a scan reports, so the dictionary path can be checked
    /// against what it claims to have counted.
    #[derive(Default)]
    struct Recorder {
        values: Vec<(Value, usize)>,
        nulls: usize,
    }

    impl super::Observe for Recorder {
        fn observe(&mut self, decoded: super::Decoded, count: usize) {
            if let super::Decoded::Value(value) = decoded {
                self.values.push((value, count));
            }
        }

        fn observe_null(&mut self, count: usize) {
            self.nulls += count;
        }
    }

    fn write_dictionary_file(
        values: &[i64],
        nulls_every: usize,
        version: WriterVersion,
    ) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ddp_profile_unit_{}_{}_{version:?}.parquet",
            std::process::id(),
            values.len()
        ));
        let schema = Arc::new(parse_message_type("message s { OPTIONAL INT64 v; }").unwrap());
        let properties = Arc::new(
            WriterProperties::builder()
                .set_dictionary_enabled(true)
                .set_writer_version(version)
                .build(),
        );
        let file = File::create(&path).unwrap();
        let mut writer = SerializedFileWriter::new(file, schema, properties).unwrap();
        let mut group = writer.next_row_group().unwrap();
        let mut column = group.next_column().unwrap().unwrap();
        let definition: Vec<i16> = (0..values.len())
            .map(|row| i16::from(nulls_every == 0 || row % nulls_every != 0))
            .collect();
        let present: Vec<i64> = values
            .iter()
            .zip(&definition)
            .filter(|(_, def)| **def == 1)
            .map(|(value, _)| *value)
            .collect();
        column
            .typed::<parquet::data_type::Int64Type>()
            .write_batch(&present, Some(&definition), None)
            .unwrap();
        column.close().unwrap();
        group.close().unwrap();
        writer.close().unwrap();
        path
    }

    /// A plain scan is always correct, so an agreement test alone could pass
    /// with the dictionary reading never aggregating anything. This pins that a
    /// dictionary-encoded chunk is observed per distinct value with a count,
    /// not once per row — for both page layouts.
    #[test]
    fn a_dictionary_chunk_is_observed_per_distinct_value() {
        for version in [WriterVersion::PARQUET_1_0, WriterVersion::PARQUET_2_0] {
            let values: Vec<i64> = (0..1_000).map(|row| row % 4).collect();
            let path = write_dictionary_file(&values, 5, version);
            let ctx = FileContext::open(&path).unwrap();
            let leaf = find_leaf(ctx.parquet().file_metadata().schema_descr(), "v").unwrap();

            let mut recorder = Recorder::default();
            scan_group(&ctx, leaf, 0, true, &mut recorder).unwrap();
            std::fs::remove_file(&path).ok();

            assert_eq!(recorder.nulls, 200, "{version:?}");
            let total: usize = recorder.values.iter().map(|(_, count)| count).sum();
            assert_eq!(total, 800, "{version:?}");
            // Far fewer observations than rows proves the indices were tallied
            // rather than each row decoded: at most one per distinct per batch.
            assert!(
                recorder.values.len() <= 4,
                "{version:?}: expected aggregated observations, got {}",
                recorder.values.len()
            );
            assert!(
                recorder.values.iter().all(|(_, count)| *count > 1),
                "{version:?}: counts must aggregate runs"
            );
        }
    }

    #[test]
    fn bins_are_half_open_except_the_first() {
        let mut bins = BinCounts::new(0.0, 20.0);
        bins.add(0.0, 1); // the minimum belongs to the first bin
        bins.add(1.0, 1); // as does its upper bound
        bins.add(1.5, 1); // above it, so the second
        bins.add(20.0, 1); // the maximum lands in the last
        let histogram = bins.finish(NotFinite::default());
        assert_eq!(histogram.bins.len(), BINS);
        assert_eq!(histogram.bins[0].count, 2);
        assert_eq!(histogram.bins[1].count, 1);
        assert_eq!(histogram.bins[BINS - 1].count, 1);
        assert!(histogram.bins[0].lower_inclusive);
        assert!(!histogram.bins[1].lower_inclusive);
        assert_eq!(histogram.bins[0].lower, 0.0);
        assert_eq!(histogram.bins[BINS - 1].upper, 20.0);
    }

    #[test]
    fn a_single_value_gets_one_bin() {
        let mut bins = BinCounts::new(7.0, 7.0);
        bins.add(7.0, 3);
        let histogram = bins.finish(NotFinite::default());
        assert_eq!(histogram.bins.len(), 1);
        assert_eq!(histogram.bins[0].count, 3);
        assert_eq!(
            (histogram.bins[0].lower, histogram.bins[0].upper),
            (7.0, 7.0)
        );
    }
}
