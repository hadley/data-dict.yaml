//! Dictionary-page fast path for enum membership (D04).
//!
//! Enums are low-cardinality and so almost always dictionary-encoded, which
//! means a column chunk's distinct values sit in one small dictionary page
//! rather than being spread across every row. Reading those pages lets the
//! common, conforming case be settled without decoding all the data.
//!
//! This proves *conformance* only. It returns `true` when every value is
//! provably in the allowed set, and `false` at the first sign of doubt — a
//! chunk that isn't fully dictionary-encoded, an unsupported physical type, or
//! a dictionary entry outside the set — leaving the caller to fall back to the
//! full value scan (which also reports the offending rows).

use std::collections::HashSet;

use parquet::basic::{PageType, Type as PhysicalType};
use parquet::column::page::{Page, PageReader};
use parquet::file::metadata::ColumnChunkMetaData;
use parquet::file::reader::FileReader;

use crate::ParquetError;
use crate::page::{for_each_plain_byte_array, is_dictionary};

/// Whether every non-null value in the `leaf`th column is provably in `allowed`,
/// determined from the row groups' dictionary pages alone. `false` means "not
/// proven" (scan to be sure), never "definitely violates".
///
/// Membership is string equality: `allowed` holds the enum's declared string
/// values, and dictionary entries are compared as UTF-8 strings.
pub(crate) fn dictionary_conforms(
    reader: &dyn FileReader,
    leaf: usize,
    allowed: &HashSet<String>,
) -> Result<bool, ParquetError> {
    let meta = reader.metadata();
    for group in 0..meta.num_row_groups() {
        let column = meta.row_group(group).column(leaf);
        if column.num_values() == 0 {
            continue;
        }
        if column.dictionary_page_offset().is_none() {
            return Ok(false);
        }
        let row_group = reader.get_row_group(group)?;
        let mut pages = row_group.get_column_page_reader(leaf)?;

        // The dictionary page comes first; it holds every value the data pages
        // reference. If any entry is outside the set, defer to the scan.
        let Some(page @ Page::DictionaryPage { .. }) = pages.get_next_page()? else {
            return Ok(false);
        };
        if !dictionary_in_set(&page, column.column_type(), allowed) {
            return Ok(false);
        }
        if !data_pages_all_dictionary(column, &mut *pages)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether every data page draws from the dictionary (so the dictionary page is
/// exhaustive). Uses the footer's page encoding stats when the writer recorded
/// them; otherwise inspects each page's encoding directly — cheap, since these
/// pages hold only dictionary indices and are never decoded here.
fn data_pages_all_dictionary(
    column: &ColumnChunkMetaData,
    pages: &mut dyn PageReader,
) -> Result<bool, ParquetError> {
    if let Some(stats) = column.page_encoding_stats() {
        let mut data_pages = 0;
        for stat in stats {
            if matches!(stat.page_type, PageType::DATA_PAGE | PageType::DATA_PAGE_V2) {
                if !is_dictionary(stat.encoding) {
                    return Ok(false);
                }
                data_pages += stat.count;
            }
        }
        return Ok(data_pages > 0);
    }
    let mut data_pages = 0;
    while let Some(page) = pages.get_next_page()? {
        if !is_dictionary(page.encoding()) {
            return Ok(false);
        }
        data_pages += 1;
    }
    Ok(data_pages > 0)
}

/// Whether every value in a PLAIN-encoded dictionary page is in `allowed`.
/// An enum column is string-like (`BYTE_ARRAY`); any other physical type
/// defers to the scan.
fn dictionary_in_set(page: &Page, physical: PhysicalType, allowed: &HashSet<String>) -> bool {
    let Page::DictionaryPage {
        buf, num_values, ..
    } = page
    else {
        return false;
    };
    let count = *num_values as usize;
    match physical {
        PhysicalType::BYTE_ARRAY => byte_arrays_in_set(buf, count, allowed),
        _ => false,
    }
}

/// Whether every PLAIN byte-array value is UTF-8 and present in `allowed`.
fn byte_arrays_in_set(buf: &[u8], count: usize, allowed: &HashSet<String>) -> bool {
    for_each_plain_byte_array(buf, count, |value| {
        std::str::from_utf8(value).is_ok_and(|text| allowed.contains(text))
    })
}
