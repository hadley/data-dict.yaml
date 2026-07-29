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

use std::fs::File;
use std::path::Path;

use parquet::basic::{Encoding, PageType};
use parquet::column::page::Page;
use parquet::file::metadata::{ColumnChunkMetaData, ParquetMetaData};
use parquet::file::reader::{FileReader, RowGroupReader, SerializedFileReader};
use parquet::file::statistics::Statistics;
use parquet::schema::types::Type;
use rayon::prelude::*;

use crate::ParquetError;
use crate::column_scan::{PlannedColumn, plan_column, read_batch};
use crate::rle::HybridDecoder;
use crate::sketch::{BottomK, HyperLogLog, SpaceSaving, hash_value};
use crate::value::{Decoded, Repr, Value, ValueKind, batch_value, classify, decode_dictionary};

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
    pub row_count: usize,
    /// Nulls in the column. For a [`ValueKind::Unsupported`] column, which is
    /// never read, this is the footer's count and falls back to 0 when the file
    /// carries no statistics.
    pub null_count: usize,
    pub distinct: Distinct,
    /// The extremes, for ordered kinds with at least one value.
    pub min: Option<Value>,
    pub max: Option<Value>,
    /// The [`TOP_VALUES`] most frequent values, most frequent first.
    pub value_counts: Vec<ValueCount>,
    /// The distribution across [`BINS`] equal-width bins, for kinds on a
    /// numeric scale that hold at least one value.
    pub histogram: Option<Histogram>,
    /// Up to [`EXAMPLES`] representative values, spread along the sorted
    /// distinct values.
    pub examples: Vec<Value>,
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

/// How often one value occurs. Once a column exceeds [`TRACKED_VALUES`]
/// distinct values, a value first seen after that point inherits the count of
/// the entry it displaced, so `count` becomes an upper bound: the true count is
/// somewhere in `count - error ..= count`. Below that threshold `error` is 0
/// and `count` is exact.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueCount {
    pub value: Value,
    pub count: usize,
    pub error: usize,
}

/// How a column's values are distributed, plus the float values that have no
/// place in that distribution.
///
/// The three counts are float-only and are all 0 otherwise. None of the values
/// they count reaches the bins, the extremes, the distinct count or the
/// examples: a NaN isn't equal to itself, and an infinity as a minimum or a
/// maximum would stretch every bin to infinite width. Counting them here keeps
/// them visible without letting them distort everything else.
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    /// Equal-width bins spanning the column's finite range, low to high. Nulls
    /// are never binned — that is `null_count`, which applies to every type.
    pub bins: Vec<Bin>,
    pub nan_count: usize,
    pub negative_infinity_count: usize,
    pub positive_infinity_count: usize,
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
pub fn profile(path: &Path, columns: Option<&[&str]>) -> Result<Vec<ColumnProfile>, ParquetError> {
    let file =
        File::open(path).map_err(|e| ParquetError::General(format!("Cannot open file: {e}")))?;
    let reader = SerializedFileReader::new(file)?;
    let meta = reader.metadata();
    let row_count = meta.file_metadata().num_rows().max(0) as usize;
    let targets = select(meta, columns)?;

    // Columns are independent, and each holds only its own bounded summary
    // while it runs, so the peak memory is set by the pool size rather than by
    // how wide the file is.
    targets
        .par_iter()
        .map(|target| {
            if target.readable() {
                profile_column(path, target, row_count)
            } else {
                Ok(stub(target, row_count, footer_nulls(meta, target.leaf)))
            }
        })
        .collect()
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
    let descr = meta.file_metadata().schema_descr_ptr();
    let target = |field: &Type| {
        let (kind, repr) = classify(field);
        Target {
            name: field.name().to_string(),
            kind,
            repr,
            leaf: plan_column(&descr, field.name()).ok().map(|c| c.leaf),
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

/// The profile of a column whose values are never read.
fn stub(target: &Target, row_count: usize, null_count: usize) -> ColumnProfile {
    ColumnProfile {
        name: target.name.clone(),
        kind: target.kind.clone(),
        row_count,
        null_count,
        distinct: Distinct::Exact(0),
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

fn profile_column(
    path: &Path,
    target: &Target,
    row_count: usize,
) -> Result<ColumnProfile, ParquetError> {
    let file =
        File::open(path).map_err(|e| ParquetError::General(format!("Cannot open file: {e}")))?;
    let reader = SerializedFileReader::new(file)?;
    let descr = reader.metadata().file_metadata().schema_descr_ptr();
    let planned = plan_column(&descr, &target.name)?;

    // Taking the range from the footer keeps the bin edges known before the
    // pass, so values can be binned as they stream by.
    let range = target
        .kind
        .is_binnable()
        .then(|| footer_range(reader.metadata(), &planned, target.repr))
        .flatten();
    let mut accumulator = Accumulator::new(target, range);
    scan(&reader, &planned, target.repr, &mut accumulator)?;

    if accumulator.unprofilable {
        let target = Target {
            name: target.name.clone(),
            kind: ValueKind::Unsupported("non-UTF-8"),
            repr: Repr::Unsupported,
            leaf: target.leaf,
        };
        // The scan stopped where the bad value was, so its running null count
        // covers only part of the column; the footer's covers all of it.
        let nulls = footer_nulls(reader.metadata(), target.leaf);
        return Ok(stub(&target, row_count, nulls));
    }

    // Without footer statistics the range is only known once the values have
    // been seen, so binning them takes a second pass. Nothing is buffered in
    // between: the first pass already reduced the column to its extremes.
    if accumulator.bins.is_none()
        && target.kind.is_binnable()
        && let Some(bins) = BinCounts::spanning(&accumulator.min, &accumulator.max)
    {
        let mut binner = BinOnly { bins };
        scan(&reader, &planned, target.repr, &mut binner)?;
        accumulator.bins = Some(binner.bins);
    }
    Ok(accumulator.finish(target, row_count))
}

/// The `[min, max]` range every row group's footer agrees on, or `None` if any
/// of them can't supply an exact one.
fn footer_range(meta: &ParquetMetaData, planned: &PlannedColumn, repr: Repr) -> Option<(f64, f64)> {
    // An unsigned column's footer extremes are ordered by the unsigned reading
    // of bits this crate stores signed, so they can't be compared as read.
    if matches!(repr, Repr::Uint(_)) {
        return None;
    }
    let mut range: Option<(f64, f64)> = None;
    for group in meta.row_groups() {
        let column = group.column(planned.leaf);
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

/// Float values that belong to no bin, tallied by what they are.
#[derive(Default, Clone, Copy)]
struct NotFinite {
    nans: usize,
    negative: usize,
    positive: usize,
}

impl NotFinite {
    fn add(&mut self, value: f64, count: usize) {
        if value.is_nan() {
            self.nans += count;
        } else if value.is_sign_negative() {
            self.negative += count;
        } else {
            self.positive += count;
        }
    }

    fn any(self) -> bool {
        self.nans + self.negative + self.positive > 0
    }
}

/// Everything a profile needs, accumulated in bounded space.
struct Accumulator {
    /// Whether the values are ordered, so tracking extremes is meaningful.
    ordered: bool,
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

    fn finish(self, target: &Target, row_count: usize) -> ColumnProfile {
        let distinct = if self.counts.is_saturated() {
            Distinct::Approx(self.distinct.estimate())
        } else {
            Distinct::Exact(self.counts.len())
        };
        let histogram = match self.bins {
            Some(bins) => Some(bins.finish(self.not_finite)),
            // No value had a place on the number line, so there is no range to
            // bin — but what was there is still worth reporting.
            None if self.not_finite.any() && target.kind.is_binnable() => {
                Some(empty_histogram(self.not_finite))
            }
            None => None,
        };
        ColumnProfile {
            name: target.name.clone(),
            kind: target.kind.clone(),
            row_count,
            null_count: self.nulls,
            distinct,
            min: self.min,
            max: self.max,
            value_counts: self
                .counts
                .top(TOP_VALUES)
                .into_iter()
                .map(|(value, count, error)| ValueCount {
                    value,
                    count,
                    error,
                })
                .collect(),
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
        let hash = hash_value(&value);
        self.distinct.insert(hash);
        self.sample.insert(hash, &value);
        self.counts.add(&value, count);
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
        Histogram {
            bins,
            ..empty_histogram(not_finite)
        }
    }
}

/// A histogram with no bins, holding only the values that belong to none.
fn empty_histogram(not_finite: NotFinite) -> Histogram {
    Histogram {
        bins: Vec::new(),
        nan_count: not_finite.nans,
        negative_infinity_count: not_finite.negative,
        positive_infinity_count: not_finite.positive,
    }
}

/// Stream one column through `observer`, one row group at a time.
fn scan(
    reader: &SerializedFileReader<File>,
    planned: &PlannedColumn,
    repr: Repr,
    observer: &mut impl Observe,
) -> Result<(), ParquetError> {
    for group in 0..reader.num_row_groups() {
        let chunk = reader.metadata().row_group(group).column(planned.leaf);
        if chunk.num_values() == 0 {
            continue;
        }
        let row_group = reader.get_row_group(group)?;
        if !count_dictionary(&*row_group, chunk, planned, repr, observer)? {
            scan_values(&*row_group, planned, repr, observer)?;
        }
        if observer.abandoned() {
            return Ok(());
        }
    }
    Ok(())
}

/// Decode every value of a column chunk. Always correct, and the fallback
/// whenever the dictionary can't answer.
fn scan_values(
    row_group: &dyn RowGroupReader,
    planned: &PlannedColumn,
    repr: Repr,
    observer: &mut impl Observe,
) -> Result<(), ParquetError> {
    let mut reader = row_group.get_column_reader(planned.leaf)?;
    loop {
        let batch = read_batch(&mut reader, planned)?;
        if batch.len() == 0 {
            return Ok(());
        }
        for row in 0..batch.len() {
            if batch.is_null(row) {
                observer.observe_null(1);
            } else {
                observer.observe(batch_value(&batch, row, repr), 1);
            }
        }
        if observer.abandoned() {
            return Ok(());
        }
    }
}

/// Summarize a dictionary-encoded column chunk from its dictionary page and the
/// indices pointing into it, without decoding a single value.
///
/// A chunk's distinct values are exactly its dictionary, and counting how often
/// each index is referenced is a matter of adding up run lengths — so a chunk
/// of a million rows over ten distinct values costs ten observations. Returns
/// `false` when the chunk isn't laid out this way, leaving the caller to scan
/// its values; nothing is reported to `observer` in that case, so the fallback
/// can't double count.
fn count_dictionary(
    row_group: &dyn RowGroupReader,
    chunk: &ColumnChunkMetaData,
    planned: &PlannedColumn,
    repr: Repr,
    observer: &mut impl Observe,
) -> Result<bool, ParquetError> {
    if chunk.dictionary_page_offset().is_none() || !encoding_stats_all_dictionary(chunk) {
        return Ok(false);
    }
    let mut pages = row_group.get_column_page_reader(planned.leaf)?;
    let Some(Page::DictionaryPage {
        buf, num_values, ..
    }) = pages.get_next_page()?
    else {
        return Ok(false);
    };
    let Some(values) = decode_dictionary(&buf, num_values as usize, planned.physical, repr) else {
        return Ok(false);
    };

    let mut counts = vec![0usize; values.len()];
    let mut nulls = 0usize;
    while let Some(page) = pages.get_next_page()? {
        if !count_page(&page, planned.max_def, &mut counts, &mut nulls) {
            return Ok(false);
        }
    }

    observer.observe_null(nulls);
    for (value, count) in values.into_iter().zip(counts) {
        if count > 0 {
            observer.observe(value, count);
            if observer.abandoned() {
                break;
            }
        }
    }
    Ok(true)
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

fn is_dictionary(encoding: Encoding) -> bool {
    matches!(
        encoding,
        Encoding::RLE_DICTIONARY | Encoding::PLAIN_DICTIONARY
    )
}

/// Add one data page's dictionary references to `counts`, and its nulls to
/// `nulls`. `false` means the page isn't dictionary-encoded or doesn't parse,
/// and the chunk must be scanned instead.
///
/// Both page versions store the indices as a bit width followed by RLE /
/// bit-packed runs; they differ in how the nulls before them are described.
fn count_page(page: &Page, max_def: i16, counts: &mut [usize], nulls: &mut usize) -> bool {
    match page {
        Page::DataPage {
            buf,
            num_values,
            encoding,
            def_level_encoding,
            ..
        } => {
            if !is_dictionary(*encoding) {
                return false;
            }
            let values = *num_values as usize;
            let mut indices = buf.as_ref();
            let mut page_nulls = 0usize;
            if max_def > 0 {
                if *def_level_encoding != Encoding::RLE {
                    return false;
                }
                // Version 1 pages prefix the levels with their byte length.
                let Some(length) = indices.get(..4) else {
                    return false;
                };
                let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
                let Some(levels) = indices.get(4..4 + length) else {
                    return false;
                };
                let mut decoder = HybridDecoder::new(levels, level_bits(max_def));
                let counted = decoder.for_each_run(values, |level, run| {
                    if level != max_def as u32 {
                        page_nulls += run;
                    }
                });
                if counted.is_err() {
                    return false;
                }
                indices = &indices[4 + length..];
            }
            if !count_indices(indices, values.saturating_sub(page_nulls), counts) {
                return false;
            }
            *nulls += page_nulls;
            true
        }
        Page::DataPageV2 {
            buf,
            num_values,
            num_nulls,
            encoding,
            def_levels_byte_len,
            rep_levels_byte_len,
            ..
        } => {
            if !is_dictionary(*encoding) {
                return false;
            }
            // Version 2 pages state their null count and the size of the level
            // sections in the header, so the indices can be found directly.
            let start = (*rep_levels_byte_len + *def_levels_byte_len) as usize;
            let Some(indices) = buf.get(start..) else {
                return false;
            };
            let values = (*num_values as usize).saturating_sub(*num_nulls as usize);
            if !count_indices(indices, values, counts) {
                return false;
            }
            *nulls += *num_nulls as usize;
            true
        }
        Page::DictionaryPage { .. } => false,
    }
}

/// Tally `values` dictionary indices, prefixed by their bit width.
fn count_indices(buf: &[u8], values: usize, counts: &mut [usize]) -> bool {
    if values == 0 {
        return true;
    }
    let Some((&bit_width, runs)) = buf.split_first() else {
        return false;
    };
    let mut within_dictionary = true;
    let counted =
        HybridDecoder::new(runs, bit_width as u32).for_each_run(values, |index, run| match counts
            .get_mut(index as usize)
        {
            Some(count) => *count += run,
            None => within_dictionary = false,
        });
    counted.is_ok() && within_dictionary
}

/// Bits needed to hold a definition level up to `max_def`.
fn level_bits(max_def: i16) -> u32 {
    u32::BITS - (max_def as u32).leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::{BINS, BinCounts, NotFinite, count_dictionary, level_bits};
    use crate::column_scan::plan_column;
    use crate::value::{Repr, Value, classify};
    use parquet::file::properties::{WriterProperties, WriterVersion};
    use parquet::file::reader::{FileReader, SerializedFileReader};
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

    /// The fallback is always correct, so an agreement test alone could pass
    /// with the dictionary path never running. This pins that it does run — for
    /// both page layouts, which describe their nulls differently.
    #[test]
    fn the_dictionary_path_counts_a_chunk_without_reading_values() {
        for version in [WriterVersion::PARQUET_1_0, WriterVersion::PARQUET_2_0] {
            let values: Vec<i64> = (0..1_000).map(|row| row % 4).collect();
            let path = write_dictionary_file(&values, 5, version);
            let reader = SerializedFileReader::new(File::open(&path).unwrap()).unwrap();
            let descr = reader.metadata().file_metadata().schema_descr_ptr();
            let planned = plan_column(&descr, "v").unwrap();
            let (_, repr) = classify(&reader.metadata().file_metadata().schema().get_fields()[0]);
            assert_eq!(repr, Repr::Int);

            let mut recorder = Recorder::default();
            let chunk = reader.metadata().row_group(0).column(planned.leaf);
            let row_group = reader.get_row_group(0).unwrap();
            let handled =
                count_dictionary(&*row_group, chunk, &planned, repr, &mut recorder).unwrap();
            std::fs::remove_file(&path).ok();

            assert!(
                handled,
                "{version:?}: a dictionary chunk must take the fast path"
            );
            assert_eq!(recorder.nulls, 200, "{version:?}");
            // Four distinct values, each reported once with its total.
            let total: usize = recorder.values.iter().map(|(_, count)| count).sum();
            assert_eq!(recorder.values.len(), 4, "{version:?}");
            assert_eq!(total, 800, "{version:?}");
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

    #[test]
    fn definition_levels_take_as_many_bits_as_the_maximum_needs() {
        assert_eq!(level_bits(0), 0);
        assert_eq!(level_bits(1), 1);
        assert_eq!(level_bits(2), 2);
        assert_eq!(level_bits(3), 2);
        assert_eq!(level_bits(4), 3);
    }
}
