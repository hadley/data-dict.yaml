//! Reading chosen columns at chosen row indices, so a diagnostic can name the
//! primary key of each offending row rather than only where the row sits.

use std::collections::HashMap;
use std::path::Path;

use arrow_array::Array;
use parquet::arrow::arrow_reader::{RowSelection, RowSelector};

use crate::ParquetError;
use crate::display::{Values, display_form};
use crate::reader::FileContext;

/// The values `columns` hold at each 0-based row index of `rows`, aligned with
/// it: `result[i][j]` is column `columns[j]` at row `rows[i]`, `None` when the
/// value is null. Rendered with the same display formatter diagnostics use.
pub fn values_at_rows(
    path: &Path,
    columns: &[String],
    rows: &[usize],
) -> Result<Vec<Vec<Option<String>>>, ParquetError> {
    if columns.is_empty() {
        return Ok(vec![Vec::new(); rows.len()]);
    }
    let ctx = FileContext::open(path)?;
    let leaves = columns
        .iter()
        .map(|name| {
            ctx.leaf(name)
                .ok_or_else(|| ParquetError::General(format!("Column not found: {name}")))
        })
        .collect::<Result<Vec<_>, _>>()?;

    // The reader yields selected rows in file order, so read the sorted set
    // into a map and answer each requested index from it.
    let mut wanted: Vec<usize> = rows.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    let mut selectors = Vec::new();
    let mut cursor = 0;
    let mut i = 0;
    while i < wanted.len() {
        let start = wanted[i];
        if start > cursor {
            selectors.push(RowSelector::skip(start - cursor));
        }
        let mut end = start;
        while i + 1 < wanted.len() && wanted[i + 1] == end + 1 {
            i += 1;
            end = wanted[i];
        }
        selectors.push(RowSelector::select(end - start + 1));
        cursor = end + 1;
        i += 1;
    }

    // A projection yields columns in leaf order, not request order; map each
    // batch column back to the position of the column it answers.
    let mut order: Vec<usize> = (0..leaves.len()).collect();
    order.sort_by_key(|&j| leaves[j]);
    let mut batch_position = vec![0; leaves.len()];
    for (position, &j) in order.iter().enumerate() {
        batch_position[j] = position;
    }

    let reader = ctx
        .builder(leaves)?
        .with_row_selection(RowSelection::from(selectors))
        .build()?;
    let mut found: HashMap<usize, Vec<Option<String>>> = HashMap::new();
    let mut next = 0;
    for batch in reader {
        let batch = batch?;
        let arrays = (0..batch.num_columns())
            .map(|j| display_form(batch.column(j)))
            .collect::<Vec<_>>();
        let formatters = arrays
            .iter()
            .map(|array| Values::new(array))
            .collect::<Result<Vec<_>, ParquetError>>()?;
        for row in 0..batch.num_rows() {
            let values = arrays
                .iter()
                .zip(&formatters)
                .map(|(array, values)| (!array.is_null(row)).then(|| values.get(row)))
                .collect();
            found.insert(wanted[next], values);
            next += 1;
        }
    }

    Ok(rows
        .iter()
        .map(|row| {
            let values = found
                .remove(row)
                .unwrap_or_else(|| vec![None; columns.len()]);
            batch_position.iter().map(|&p| values[p].clone()).collect()
        })
        .collect())
}
