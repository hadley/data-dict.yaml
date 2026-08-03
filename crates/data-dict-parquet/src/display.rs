//! Human-readable value rendering for diagnostic samples, via arrow's
//! type-aware formatter (dates render as dates, decimals at their scale).

use arrow_array::Array;
use arrow_cast::display::{ArrayFormatter, FormatOptions};

use crate::ParquetError;

pub(crate) struct Values<'a> {
    formatter: ArrayFormatter<'a>,
}

impl<'a> Values<'a> {
    pub(crate) fn new(array: &'a dyn Array) -> Result<Self, ParquetError> {
        Ok(Values {
            formatter: ArrayFormatter::try_new(array, &FormatOptions::default())?,
        })
    }

    /// The rendered value at `row`. Only meaningful for non-null rows.
    pub(crate) fn get(&self, row: usize) -> String {
        self.formatter.value(row).to_string()
    }
}
