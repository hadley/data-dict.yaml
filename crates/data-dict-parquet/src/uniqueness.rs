use std::path::Path;

use arrow_array::{Array, ArrayRef, RecordBatch};
use arrow_row::{RowConverter, SortField};
use rayon::prelude::*;

use crate::ParquetError;
use crate::display::{Values, display_form};
use crate::keys::{ByteKeys, KeyColumn, KeySet, canonicalize};
use crate::reader::FileContext;

#[derive(Clone)]
pub struct UniquenessCheck {
    pub columns: Vec<String>,
}

/// The duplicates a [`UniquenessCheck`] found. `duplicate_count` is the total;
/// `duplicate_rows` samples the first few repeat occurrences (1-based) and
/// `duplicate_values` the key each of those rows held, rendered in
/// `key_columns` order. A value is rendered from the column as stored, not from
/// the canonicalized form the comparison uses, so `-0.0` reads as it was
/// written even though it duplicates `0.0`.
#[derive(Default)]
pub struct UniquenessStats {
    pub duplicate_count: usize,
    pub duplicate_rows: Vec<usize>,
    pub duplicate_values: Vec<Vec<String>>,
    /// The key's columns, in key order and without the repeats a caller may
    /// have listed.
    pub key_columns: Vec<String>,
}

/// Validate uniqueness exactly by hashing each row's key in a streaming,
/// column-projected pass over the file.
///
/// Memory is proportional to the number of distinct keys: a duplicate is a key
/// already seen. A single scalar-like column is hashed without allocating per
/// value; a single byte column is hashed by its value bytes directly (a slice
/// into the decoded batch); a composite key is hashed by its `arrow-row`
/// encoding, whose length-framing keeps columns from colliding across splits.
/// Checks are independent and run in parallel, each reading only the columns
/// it needs.
pub fn uniqueness_stats(
    path: &Path,
    checks: &[UniquenessCheck],
    sample_limit: usize,
) -> Result<Vec<UniquenessStats>, ParquetError> {
    checks
        .par_iter()
        .map(|check| check_uniqueness(path, check, sample_limit))
        .collect()
}

fn check_uniqueness(
    path: &Path,
    check: &UniquenessCheck,
    sample_limit: usize,
) -> Result<UniquenessStats, ParquetError> {
    let ctx = FileContext::open(path)?;
    // A column repeated within a key adds no discriminating power; read it once.
    let mut names: Vec<&str> = Vec::new();
    for name in &check.columns {
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    let leaves = names
        .iter()
        .map(|name| {
            ctx.leaf(name)
                .ok_or_else(|| ParquetError::General(format!("Column not found: {name}")))
        })
        .collect::<Result<Vec<_>, _>>()?;

    // A uniqueness check expects mostly-distinct keys, so size for one entry
    // per row up front.
    let rows = ctx.rows();
    let reader = ctx.reader(leaves)?;
    let mut stat = UniquenessStats {
        key_columns: names.iter().map(|name| (*name).to_string()).collect(),
        ..UniquenessStats::default()
    };
    let mut row_offset = 0usize;

    if let [name] = names.as_slice() {
        let mut seen: Option<KeySet> = None;
        for batch in reader {
            let batch = batch?;
            let stored = batch.column_by_name(name).expect("projected column");
            let array = canonicalize(stored);
            let keys = KeyColumn::new(&array)?;
            let seen = seen.get_or_insert_with(|| KeySet::for_column(&keys, rows));
            // Hoisted out of the row loop: `Array::is_null` is a virtual call.
            let validity = array.nulls();
            let mut display: Option<Vec<ArrayRef>> = None;
            for row in 0..array.len() {
                if validity.is_some_and(|v| v.is_null(row)) {
                    continue;
                }
                if !seen.insert(&keys, row) {
                    let key = if stat.duplicate_rows.len() < sample_limit {
                        sample_key(&batch, std::slice::from_ref(name), &mut display, row)?
                    } else {
                        Vec::new()
                    };
                    stat.duplicate(row_offset + row + 1, key, sample_limit);
                }
            }
            row_offset += batch.num_rows();
        }
    } else {
        let mut converter: Option<RowConverter> = None;
        let mut seen = ByteKeys::with_capacity(rows);
        for batch in reader {
            let batch = batch?;
            let arrays: Vec<ArrayRef> = names
                .iter()
                .map(|name| canonicalize(batch.column_by_name(name).expect("projected column")))
                .collect();
            if converter.is_none() {
                converter = Some(RowConverter::new(
                    arrays
                        .iter()
                        .map(|array| SortField::new(array.data_type().clone()))
                        .collect(),
                )?);
            }
            let encoded = converter
                .as_ref()
                .expect("converter initialized above")
                .convert_columns(&arrays)?;
            // A row with a null in any key column is never compared, matching
            // SQL uniqueness (multiple nulls are allowed) and avoiding a
            // spurious D02 alongside the D01 a required column already draws.
            let validities: Vec<_> = arrays.iter().map(|array| array.nulls()).collect();
            let mut display: Option<Vec<ArrayRef>> = None;
            'row: for row in 0..batch.num_rows() {
                for validity in &validities {
                    if validity.is_some_and(|v| v.is_null(row)) {
                        continue 'row;
                    }
                }
                if !seen.insert(encoded.row(row).as_ref()) {
                    let key = if stat.duplicate_rows.len() < sample_limit {
                        sample_key(&batch, &names, &mut display, row)?
                    } else {
                        Vec::new()
                    };
                    stat.duplicate(row_offset + row + 1, key, sample_limit);
                }
            }
            row_offset += batch.num_rows();
        }
    }
    Ok(stat)
}

/// Render the key at `row`, deriving the batch's display arrays on first use.
/// Deliberately out of line: the scan loop reaches it only for a duplicate, and
/// only while the sample can still grow, so keeping it out of the loop body
/// leaves the scan itself as it was before values were reported.
#[inline(never)]
fn sample_key(
    batch: &RecordBatch,
    names: &[&str],
    display: &mut Option<Vec<ArrayRef>>,
    row: usize,
) -> Result<Vec<String>, ParquetError> {
    let arrays = match display {
        Some(arrays) => arrays,
        None => display.insert(
            names
                .iter()
                .map(|name| display_form(batch.column_by_name(name).expect("projected column")))
                .collect(),
        ),
    };
    arrays
        .iter()
        .map(|array| Ok(Values::new(array.as_ref())?.get(row)))
        .collect()
}

impl UniquenessStats {
    /// Record the duplicate at 1-based `row`, sampling it and its rendered `key`
    /// up to `sample_limit`. The caller only renders a key while the sample can
    /// still grow, and passes an empty one once it can't.
    fn duplicate(&mut self, row: usize, key: Vec<String>, sample_limit: usize) {
        self.duplicate_count += 1;
        if self.duplicate_rows.len() < sample_limit {
            self.duplicate_rows.push(row);
            self.duplicate_values.push(key);
        }
    }
}
