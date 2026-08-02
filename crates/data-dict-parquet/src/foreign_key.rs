//! Foreign-key referential integrity (D05/D06): every non-null value in a
//! child column must appear in the parent's primary-key column.
//!
//! Each check builds a set of the parent column's values in one streaming pass,
//! then streams the child column probing membership. Nulls are exempt on both
//! sides (a null foreign key references nothing). The two columns need not
//! share a physical representation: both sides are cast to a common comparison
//! type derived from the comparable-type semantics of `site/validation.md`,
//! and when no common form exists, nothing can match. If either column's type
//! can't be compared at all the check is skipped and reported as D06.

use std::path::PathBuf;

use arrow_array::{Array, ArrayRef};
use arrow_cast::cast;
use arrow_schema::{DataType, TimeUnit};
use rayon::prelude::*;

use crate::ParquetError;
use crate::display::Values;
use crate::keys::{KeyColumn, KeySet, canonicalize};
use crate::metadata::Comparability;
use crate::reader::FileContext;

/// One foreign-key check: the child column's values must all appear in the
/// parent column. The two may live in the same file (a self-join) or different
/// files.
#[derive(Clone)]
pub struct ForeignKeyCheck {
    pub child_path: PathBuf,
    pub child_column: String,
    pub parent_path: PathBuf,
    pub parent_column: String,
}

/// The outcome of a [`ForeignKeyCheck`].
pub enum ForeignKeyResult {
    /// The child or parent column uses a type whose values can't be compared, so
    /// the reference was not checked (D06). `reason` is a barrier slug (e.g.
    /// `json`).
    NotVerified { reason: &'static str },
    /// The reference was checked; `orphan_count == 0` means it holds.
    Checked(ForeignKeyStats),
}

/// Values found in the child column that are absent from the parent column.
/// `orphan_rows`/`orphan_values` sample the first few (1-based rows, distinct
/// values); `orphan_count` is the total.
#[derive(Default)]
pub struct ForeignKeyStats {
    pub orphan_count: usize,
    pub orphan_rows: Vec<usize>,
    pub orphan_values: Vec<String>,
}

/// Run every foreign-key check in parallel, each reading only its two columns.
pub fn foreign_key_stats(
    checks: &[ForeignKeyCheck],
    sample_limit: usize,
) -> Result<Vec<ForeignKeyResult>, ParquetError> {
    checks
        .par_iter()
        .map(|check| check_foreign_key(check, sample_limit))
        .collect()
}

fn check_foreign_key(
    check: &ForeignKeyCheck,
    sample_limit: usize,
) -> Result<ForeignKeyResult, ParquetError> {
    let parent = FileContext::open(&check.parent_path)?;
    let parent_leaf = column(&parent, &check.parent_column)?;
    if let Comparability::Incomparable(reason) =
        crate::metadata::uniqueness_comparability(parent.leaf_descr(parent_leaf).self_type())
    {
        return Ok(ForeignKeyResult::NotVerified { reason });
    }

    let child = FileContext::open(&check.child_path)?;
    let child_leaf = column(&child, &check.child_column)?;
    if let Comparability::Incomparable(reason) =
        crate::metadata::uniqueness_comparability(child.leaf_descr(child_leaf).self_type())
    {
        return Ok(ForeignKeyResult::NotVerified { reason });
    }

    let mut stats = ForeignKeyStats::default();
    let target = common_type(
        &parent.arrow_type(&check.parent_column)?,
        &child.arrow_type(&check.child_column)?,
    );
    let Some(target) = target else {
        // No common comparable form (say, a string referencing a number):
        // nothing can match, so every non-null child value is an orphan.
        let mut row_offset = 0usize;
        for batch in child.reader([child_leaf])? {
            let batch = batch?;
            let array = batch.column(0);
            orphan_all(array, row_offset, &mut stats, sample_limit)?;
            row_offset += batch.num_rows();
        }
        return Ok(ForeignKeyResult::Checked(stats));
    };

    let mut seen: Option<KeySet> = None;
    for batch in parent.reader([parent_leaf])? {
        let batch = batch?;
        let array = canonicalize(&cast(batch.column(0), &target)?);
        let keys = KeyColumn::new(&array)?;
        let seen = seen.get_or_insert_with(|| KeySet::for_column(&keys, parent.rows()));
        // Hoisted out of the row loops here and below: `Array::is_null` is a
        // virtual call.
        let validity = array.nulls();
        for row in 0..array.len() {
            if !validity.is_some_and(|v| v.is_null(row)) {
                seen.insert(&keys, row);
            }
        }
    }

    let mut row_offset = 0usize;
    for batch in child.reader([child_leaf])? {
        let batch = batch?;
        let original = batch.column(0);
        let casted = canonicalize(&cast(original, &target)?);
        let keys = KeyColumn::new(&casted)?;
        let seen = seen.get_or_insert_with(|| KeySet::for_column(&keys, 0));
        let mut values: Option<Values> = None;
        let original_validity = original.nulls();
        let casted_validity = casted.nulls();
        for row in 0..original.len() {
            if original_validity.is_some_and(|v| v.is_null(row)) {
                continue;
            }
            // A non-null value the cast couldn't represent in the comparison
            // type can't match anything either.
            if !casted_validity.is_some_and(|v| v.is_null(row)) && seen.contains(&keys, row) {
                continue;
            }
            let values = match &mut values {
                Some(values) => values,
                None => values.insert(Values::new(original.as_ref())?),
            };
            stats.orphan(row_offset + row, values.get(row), sample_limit);
        }
        row_offset += batch.num_rows();
    }
    Ok(ForeignKeyResult::Checked(stats))
}

fn column(ctx: &FileContext, name: &str) -> Result<usize, ParquetError> {
    ctx.leaf(name)
        .ok_or_else(|| ParquetError::General(format!("Column not found: {name}")))
}

/// Count every non-null value as an orphan (the no-common-form case).
fn orphan_all(
    array: &ArrayRef,
    row_offset: usize,
    stats: &mut ForeignKeyStats,
    sample_limit: usize,
) -> Result<(), ParquetError> {
    if array.len() == array.null_count() {
        return Ok(());
    }
    let values = Values::new(array.as_ref())?;
    for row in 0..array.len() {
        if !array.is_null(row) {
            stats.orphan(row_offset + row, values.get(row), sample_limit);
        }
    }
    Ok(())
}

impl ForeignKeyStats {
    /// Record the orphan at 0-based `row`, sampling its 1-based row number and
    /// distinct rendered value up to `sample_limit`.
    fn orphan(&mut self, row: usize, value: String, sample_limit: usize) {
        self.orphan_count += 1;
        if self.orphan_rows.len() < sample_limit {
            self.orphan_rows.push(row + 1);
        }
        if self.orphan_values.len() < sample_limit && !self.orphan_values.contains(&value) {
            self.orphan_values.push(value);
        }
    }
}

/// The comparison type both sides are cast to, derived from the spec's
/// comparable-type semantics — values are compared as values, whatever their
/// physical representation. `None` means the two types have no common
/// comparable form, so no value can match.
fn common_type(a: &DataType, b: &DataType) -> Option<DataType> {
    use DataType::*;
    if a == b {
        return Some(a.clone());
    }
    match (family(a), family(b)) {
        (Family::SignedInt, Family::SignedInt) => Some(Int64),
        (Family::UnsignedInt, Family::UnsignedInt) => Some(UInt64),
        // Mixed signedness needs a type that holds both ranges exactly.
        (Family::SignedInt | Family::UnsignedInt, Family::SignedInt | Family::UnsignedInt) => {
            Some(Decimal128(20, 0))
        }
        (Family::Float, Family::Float | Family::SignedInt | Family::UnsignedInt)
        | (Family::SignedInt | Family::UnsignedInt, Family::Float) => Some(Float64),
        (Family::Decimal(p1, s1), Family::Decimal(p2, s2)) => decimal_common((p1, s1), (p2, s2)),
        (Family::Decimal(p, s), Family::SignedInt | Family::UnsignedInt)
        | (Family::SignedInt | Family::UnsignedInt, Family::Decimal(p, s)) => {
            decimal_common((p, s), (20, 0))
        }
        (Family::Decimal(_, _), Family::Float) | (Family::Float, Family::Decimal(_, _)) => {
            Some(Float64)
        }
        (Family::Str, Family::Str) => Some(LargeUtf8),
        (Family::Bytes, Family::Bytes) => Some(LargeBinary),
        (Family::Date, Family::Date) => Some(Date64),
        // Compare instants at the finer unit; the timezone label doesn't
        // change the stored value.
        (Family::Timestamp(u1), Family::Timestamp(u2)) => Some(Timestamp(finer(u1, u2), None)),
        (Family::Time, Family::Time) => Some(Time64(TimeUnit::Nanosecond)),
        _ => None,
    }
}

enum Family {
    SignedInt,
    UnsignedInt,
    Float,
    Decimal(u8, i8),
    Str,
    Bytes,
    Date,
    Timestamp(TimeUnit),
    Time,
    Other,
}

fn family(t: &DataType) -> Family {
    use DataType::*;
    match t {
        Int8 | Int16 | Int32 | Int64 => Family::SignedInt,
        UInt8 | UInt16 | UInt32 | UInt64 => Family::UnsignedInt,
        Float16 | Float32 | Float64 => Family::Float,
        Decimal128(p, s) | Decimal256(p, s) => Family::Decimal(*p, *s),
        Utf8 | LargeUtf8 | Utf8View => Family::Str,
        Binary | LargeBinary | BinaryView | FixedSizeBinary(_) => Family::Bytes,
        Date32 | Date64 => Family::Date,
        Timestamp(unit, _) => Family::Timestamp(*unit),
        Time32(_) | Time64(_) => Family::Time,
        _ => Family::Other,
    }
}

/// The narrowest decimal type holding both `(precision, scale)` shapes exactly.
fn decimal_common((p1, s1): (u8, i8), (p2, s2): (u8, i8)) -> Option<DataType> {
    let scale = s1.max(s2) as i16;
    let p1 = p1 as i16 + (scale - s1 as i16);
    let p2 = p2 as i16 + (scale - s2 as i16);
    let precision = p1.max(p2);
    if precision <= 38 {
        Some(DataType::Decimal128(precision as u8, scale as i8))
    } else if precision <= 76 {
        Some(DataType::Decimal256(precision as u8, scale as i8))
    } else {
        None
    }
}

fn finer(a: TimeUnit, b: TimeUnit) -> TimeUnit {
    fn rank(unit: TimeUnit) -> u8 {
        match unit {
            TimeUnit::Second => 0,
            TimeUnit::Millisecond => 1,
            TimeUnit::Microsecond => 2,
            TimeUnit::Nanosecond => 3,
        }
    }
    if rank(a) >= rank(b) { a } else { b }
}
