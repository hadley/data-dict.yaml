//! Human-readable value rendering for diagnostic samples, via arrow's
//! type-aware formatter (dates render as dates, decimals at their scale).

use arrow_array::{Array, ArrayRef};
use arrow_cast::cast;
use arrow_cast::display::{ArrayFormatter, FormatOptions};
use arrow_schema::DataType;

use crate::ParquetError;

/// The form a column is rendered from. A timestamp's timezone label is dropped,
/// leaving the instant it names: the label doesn't change the stored value, and
/// naming a zone needs a timezone database arrow is not built with. Every other
/// array renders as it stands.
pub(crate) fn display_form(array: &ArrayRef) -> ArrayRef {
    match array.data_type() {
        DataType::Timestamp(unit, Some(_)) => {
            let naive = DataType::Timestamp(*unit, None);
            cast(array, &naive).unwrap_or_else(|_| array.clone())
        }
        _ => array.clone(),
    }
}

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
