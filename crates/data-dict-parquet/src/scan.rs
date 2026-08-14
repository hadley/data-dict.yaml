use std::collections::HashSet;
use std::path::Path;

use arrow_array::Array;
use arrow_array::cast::AsArray;
use arrow_buffer::OffsetBuffer;
use parquet::file::reader::SerializedFileReader;

use crate::ParquetError;
use crate::reader::FileContext;

/// What a column's data must be inspected for.
#[derive(Default, Clone)]
pub struct ColumnNeeds {
    /// Count nulls and sample the row numbers where they occur. Nulls are
    /// counted on the top-level column itself — for a nested column that is
    /// the container, so an empty list doesn't count.
    pub nulls: bool,
    /// The set of allowed values (D04). When present, non-null values not in
    /// the set are counted and sampled. Membership is string equality: the
    /// metadata level guarantees an enum column is string-like (M01).
    pub allowed: Option<HashSet<String>>,
}

impl ColumnNeeds {
    pub fn any(&self) -> bool {
        self.nulls || self.allowed.is_some()
    }

    pub fn merge(self, other: Self) -> Self {
        ColumnNeeds {
            nulls: self.nulls || other.nulls,
            allowed: self.allowed.or(other.allowed),
        }
    }
}

/// One column (or nested field) to inspect: where it is, and what for. The
/// path is a top-level column name followed by struct field names; values
/// inside lists are reached automatically (`list(enum)` needs no extra
/// segment).
pub struct ColumnRequest {
    pub path: Vec<String>,
    pub needs: ColumnNeeds,
}

/// Statistics gathered by scanning a column's values.
#[derive(Default)]
pub struct ColumnStats {
    pub null_count: usize,
    /// 1-based row numbers, capped by the caller's limit.
    pub null_rows: Vec<usize>,
    /// Non-null values found outside the [`ColumnNeeds::allowed`] set.
    pub outside_count: usize,
    /// 1-based row numbers of outside values, capped by the caller's limit.
    /// A value inside a list or struct is attributed to the row holding it.
    pub outside_rows: Vec<usize>,
    /// The value each sampled row in `outside_rows` held, in the same order.
    pub outside_values: Vec<String>,
}

/// Gather requested statistics in one projected, streaming pass over the file.
/// Returns one [`ColumnStats`] per request, in request order; a request whose
/// path doesn't resolve in the file comes back untouched (all zeros).
pub fn column_stats(
    path: &Path,
    requests: &[ColumnRequest],
    limit: usize,
) -> Result<Vec<ColumnStats>, ParquetError> {
    let ctx = FileContext::open(path)?;

    let mut stats: Vec<ColumnStats> = requests.iter().map(|_| ColumnStats::default()).collect();

    let requested: Vec<(usize, usize)> = requests
        .iter()
        .enumerate()
        .filter(|(_, r)| r.needs.any())
        .filter_map(|(i, r)| ctx.leaf_path(&r.path).map(|leaf| (i, leaf)))
        .collect();

    // Fast path: settle the enum-membership need (D04) from dictionary pages
    // where the data conforms, sparing those columns the value scan. A column
    // still scanned for its nulls skips the redundant dictionary read.
    let mut proven: HashSet<usize> = HashSet::new();
    let candidates: Vec<&(usize, usize)> = requested
        .iter()
        .filter(|(i, _)| requests[*i].needs.allowed.is_some() && !requests[*i].needs.nulls)
        .collect();
    if !candidates.is_empty() {
        let page_reader = SerializedFileReader::new(ctx.file()?)?;
        for (i, leaf) in candidates {
            let allowed = requests[*i].needs.allowed.as_ref().expect("filtered");
            if crate::dictionary::dictionary_conforms(&page_reader, *leaf, allowed)
                .is_ok_and(|conforms| conforms)
            {
                proven.insert(*i);
            }
        }
    }

    let scanned: Vec<&(usize, usize)> = requested
        .iter()
        .filter(|(i, _)| {
            let need = &requests[*i].needs;
            need.nulls || (need.allowed.is_some() && !proven.contains(i))
        })
        .collect();
    if scanned.is_empty() {
        return Ok(stats);
    }

    let reader = ctx.reader(scanned.iter().map(|(_, leaf)| *leaf))?;
    let mut row_offset = 0usize;
    for batch in reader {
        let batch = batch?;
        for (i, _) in &scanned {
            let request = &requests[*i];
            let Some(array) = batch.column_by_name(&request.path[0]) else {
                continue;
            };
            let stat = &mut stats[*i];
            if request.needs.nulls
                && array.null_count() > 0
                && let Some(validity) = array.nulls()
            {
                for row in 0..array.len() {
                    if !validity.is_valid(row) {
                        stat.null_count += 1;
                        if stat.null_rows.len() < limit {
                            stat.null_rows.push(row_offset + row + 1);
                        }
                    }
                }
            }
            if let Some(allowed) = &request.needs.allowed
                && !proven.contains(i)
                && let Some((values, offsets)) = navigate(array.as_ref(), &request.path[1..])
            {
                let row_of = |index: usize| row_of(&offsets, index);
                // A string-like column decodes as strings, or — for a true
                // parquet ENUM — as binary; both hold UTF-8 values.
                if let Some(strings) = values.as_string_opt::<i32>() {
                    let values = strings.iter().map(|value| value.map(str::as_bytes));
                    check_membership(values, allowed, stat, row_offset, &row_of, limit);
                } else if let Some(bytes) = values.as_binary_opt::<i32>() {
                    check_membership(bytes.iter(), allowed, stat, row_offset, &row_of, limit);
                }
            }
        }
        row_offset += batch.num_rows();
    }

    Ok(stats)
}

/// Descend from a decoded top-level column to the values at `fields` (struct
/// field names), unwrapping list layers wherever they appear — so `list(enum)`
/// yields its elements, and a field of `list(struct)` its per-element values.
/// Returns the flat values array with the offset layers crossed, which
/// [`row_of`] uses to attribute a flat element back to its row.
fn navigate<'a>(
    root: &'a dyn Array,
    fields: &[String],
) -> Option<(&'a dyn Array, Vec<OffsetBuffer<i32>>)> {
    let mut current = root;
    let mut offsets = Vec::new();
    let mut fields = fields.iter();
    loop {
        if let Some(list) = current.as_list_opt::<i32>() {
            offsets.push(list.offsets().clone());
            current = list.values().as_ref();
            continue;
        }
        match fields.next() {
            None => return Some((current, offsets)),
            Some(field) => current = current.as_struct_opt()?.column_by_name(field)?.as_ref(),
        }
    }
}

/// The batch row holding flat element `index`, mapped up through the list
/// offset layers [`navigate`] crossed (identity when it crossed none).
fn row_of(offsets: &[OffsetBuffer<i32>], mut index: usize) -> usize {
    for layer in offsets.iter().rev() {
        // The slot whose [start, end) range contains `index`; empty slots
        // share their start with the next, and the partition point skips them.
        index = layer.partition_point(|&end| (end as usize) <= index) - 1;
    }
    index
}

/// Test each non-null value's membership in `allowed`, recording the outsiders
/// row by row, so a value that offends twice is recorded twice. Values are
/// UTF-8 bytes; one that isn't valid UTF-8 can't be in the set (the set holds
/// strings) and is rendered as hex for the sample.
fn check_membership<'a>(
    values: impl Iterator<Item = Option<&'a [u8]>>,
    allowed: &HashSet<String>,
    stat: &mut ColumnStats,
    row_offset: usize,
    row_of: &dyn Fn(usize) -> usize,
    limit: usize,
) {
    for (index, value) in values.enumerate() {
        let Some(bytes) = value else {
            continue;
        };
        let value = std::str::from_utf8(bytes).ok();
        if value.is_some_and(|value| allowed.contains(value)) {
            continue;
        }
        stat.outside_count += 1;
        if stat.outside_rows.len() < limit {
            stat.outside_rows.push(row_offset + row_of(index) + 1);
            stat.outside_values.push(match value {
                Some(value) => value.to_string(),
                None => bytes.iter().map(|b| format!("{b:02x}")).collect(),
            });
        }
    }
}
