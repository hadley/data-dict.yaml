use std::collections::{HashMap, HashSet};
use std::path::Path;

use arrow_array::Array;
use arrow_array::cast::AsArray;
use parquet::file::reader::SerializedFileReader;

use crate::ParquetError;
use crate::reader::FileContext;

/// What a column's data must be inspected for.
#[derive(Default, Clone)]
pub struct ColumnNeeds {
    /// Count nulls and sample the row numbers where they occur.
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

/// Statistics gathered by scanning a column's values.
#[derive(Default)]
pub struct ColumnStats {
    pub null_count: usize,
    /// 1-based row numbers, capped by the caller's limit.
    pub null_rows: Vec<usize>,
    /// Non-null values found outside the [`ColumnNeeds::allowed`] set.
    pub outside_count: usize,
    /// 1-based row numbers of outside values, capped by the caller's limit.
    pub outside_rows: Vec<usize>,
    /// Distinct offending values, capped by the caller's limit, in first-seen
    /// order.
    pub outside_values: Vec<String>,
}

/// Gather requested statistics in one projected, streaming pass over the file.
pub fn column_stats(
    path: &Path,
    needs: &HashMap<String, ColumnNeeds>,
    limit: usize,
) -> Result<HashMap<String, ColumnStats>, ParquetError> {
    let ctx = FileContext::open(path)?;

    let requested: Vec<(String, usize, &ColumnNeeds)> = needs
        .iter()
        .filter(|(_, need)| need.any())
        .filter_map(|(name, need)| ctx.leaf(name).map(|leaf| (name.clone(), leaf, need)))
        .collect();

    let mut stats: HashMap<String, ColumnStats> = requested
        .iter()
        .map(|(name, _, _)| (name.clone(), ColumnStats::default()))
        .collect();

    // Fast path: settle the enum-membership need (D04) from dictionary pages
    // where the data conforms, sparing those columns the value scan. A column
    // still scanned for its nulls skips the redundant dictionary read.
    let candidates: Vec<&(String, usize, &ColumnNeeds)> = requested
        .iter()
        .filter(|(_, _, need)| need.allowed.is_some() && !need.nulls)
        .collect();
    let mut proven: HashSet<&str> = HashSet::new();
    if !candidates.is_empty() {
        let page_reader = SerializedFileReader::new(ctx.file()?)?;
        for (name, leaf, need) in candidates {
            let allowed = need.allowed.as_ref().expect("filtered on allowed");
            if crate::dictionary::dictionary_conforms(&page_reader, *leaf, allowed)
                .is_ok_and(|conforms| conforms)
            {
                proven.insert(name.as_str());
            }
        }
    }

    let scanned: Vec<&(String, usize, &ColumnNeeds)> = requested
        .iter()
        .filter(|(name, _, need)| {
            need.nulls || (need.allowed.is_some() && !proven.contains(name.as_str()))
        })
        .collect();
    if scanned.is_empty() {
        return Ok(stats);
    }

    let reader = ctx.reader(scanned.iter().map(|(_, leaf, _)| *leaf))?;
    let mut row_offset = 0usize;
    for batch in reader {
        let batch = batch?;
        for (name, _, need) in &scanned {
            let Some(array) = batch.column_by_name(name) else {
                continue;
            };
            let stat = stats.get_mut(name).expect("stats entry per request");
            if need.nulls
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
            if let Some(allowed) = &need.allowed
                && !proven.contains(name.as_str())
                && let Some(strings) = array.as_string_opt::<i32>()
            {
                for row in 0..strings.len() {
                    if strings.is_null(row) {
                        continue;
                    }
                    let value = strings.value(row);
                    if !allowed.contains(value) {
                        stat.outside_count += 1;
                        if stat.outside_rows.len() < limit {
                            stat.outside_rows.push(row_offset + row + 1);
                        }
                        if stat.outside_values.len() < limit
                            && !stat.outside_values.iter().any(|v| v == value)
                        {
                            stat.outside_values.push(value.to_string());
                        }
                    }
                }
            }
        }
        row_offset += batch.num_rows();
    }

    Ok(stats)
}
