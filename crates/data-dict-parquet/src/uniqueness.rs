use std::path::Path;

use arrow_array::{Array, ArrayRef};
use arrow_row::{RowConverter, SortField};
use rayon::prelude::*;

use crate::ParquetError;
use crate::keys::{ByteKeys, KeyColumn, KeySet, canonicalize};
use crate::reader::FileContext;

#[derive(Clone)]
pub struct UniquenessCheck {
    pub columns: Vec<String>,
}

#[derive(Default)]
pub struct UniquenessStats {
    pub duplicate_count: usize,
    pub duplicate_rows: Vec<usize>,
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
    let mut stat = UniquenessStats::default();
    let mut row_offset = 0usize;

    if let [name] = names.as_slice() {
        let mut seen: Option<KeySet> = None;
        for batch in reader {
            let batch = batch?;
            let array = batch.column_by_name(name).expect("projected column");
            let array = canonicalize(array);
            let keys = KeyColumn::new(&array)?;
            let seen = seen.get_or_insert_with(|| KeySet::for_column(&keys, rows));
            // Hoisted out of the row loop: `Array::is_null` is a virtual call.
            let validity = array.nulls();
            for row in 0..array.len() {
                if validity.is_some_and(|v| v.is_null(row)) {
                    continue;
                }
                if !seen.insert(&keys, row) {
                    record(&mut stat, row_offset + row + 1, sample_limit);
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
            'row: for row in 0..batch.num_rows() {
                for validity in &validities {
                    if validity.is_some_and(|v| v.is_null(row)) {
                        continue 'row;
                    }
                }
                if !seen.insert(encoded.row(row).as_ref()) {
                    record(&mut stat, row_offset + row + 1, sample_limit);
                }
            }
            row_offset += batch.num_rows();
        }
    }
    Ok(stat)
}

fn record(stat: &mut UniquenessStats, row: usize, sample_limit: usize) {
    stat.duplicate_count += 1;
    if stat.duplicate_rows.len() < sample_limit {
        stat.duplicate_rows.push(row);
    }
}
