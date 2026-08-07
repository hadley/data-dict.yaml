//! Decoding columns as the types an expression expects.
//!
//! The one thing this crate does for assertion evaluation, and it knows nothing
//! about assertions: a caller names a set of columns and the type it wants each
//! read as, and gets back typed vectors plus validity, batch by batch.
//!
//! Asking for a type is also asking whether the data can supply it.
//! [`decodable`] answers that from the footer alone, before anything is read,
//! so a caller can report a column it can't use rather than discovering it
//! mid-scan (data-dict reports it as `D08`; see `site/validation.md`).

use std::path::Path;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Date32Type, Float64Type, Int64Type, TimestampMicrosecondType};
use arrow_array::{Array, ArrayRef, RecordBatch};
use arrow_buffer::NullBuffer;
use arrow_cast::{CastOptions, cast_with_options};
use arrow_schema::{DataType, TimeUnit};
use parquet::arrow::arrow_reader::ParquetRecordBatchReader;

use crate::ParquetError;
use crate::reader::FileContext;

/// The types a column can be asked for: the language's scalar types, minus the
/// ones no column can hold. Mirrors data-dict's expression types, which this
/// crate deliberately doesn't depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Number,
    String,
    Bool,
    Date,
    Datetime,
}

/// One column to read, and the type to read it as.
#[derive(Debug, Clone)]
pub struct TypedColumnRequest {
    /// The top-level column name, then one struct field name per dot.
    pub path: Vec<String>,
    pub ty: ValueType,
}

/// Whether a request can be met, and if not, why in one short phrase — used as
/// the "found" half of the caller's diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decodable {
    Yes,
    No(&'static str),
}

/// Can each request be met? Reads the footer only, so it is cheap enough to run
/// before deciding what to scan.
pub fn decodable(
    path: &Path,
    requests: &[TypedColumnRequest],
) -> Result<Vec<Decodable>, ParquetError> {
    let ctx = FileContext::open(path)?;
    Ok(requests.iter().map(|r| classify(&ctx, r)).collect())
}

fn classify(ctx: &FileContext, request: &TypedColumnRequest) -> Decodable {
    let Some(source) = ctx.arrow_type_path(&request.path) else {
        // Either the path doesn't exist, or it runs through a list — a value
        // per element, where an expression needs one per row.
        return Decodable::No("no such column, or it is inside a list");
    };
    match decode_target(&source, request.ty) {
        Ok(_) => Decodable::Yes,
        Err(why) => Decodable::No(why),
    }
}

/// The arrow type a source column is cast to in order to read it as `want`, or
/// why it can't be. [`decodable`] and [`read_typed`] both go through here, so
/// the promise made from the footer is the one the scan keeps.
fn decode_target(source: &DataType, want: ValueType) -> Result<DataType, &'static str> {
    // A dictionary-encoded column is its value type as far as this is concerned.
    if let DataType::Dictionary(_, values) = source {
        return decode_target(values, want);
    }
    match want {
        ValueType::Number => match source {
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32 => Ok(DataType::Int64),
            // 64-bit unsigned values run past the language's integers, and
            // quietly turning the large ones into nulls or floats would be
            // worse than saying so.
            DataType::UInt64 => Err("64-bit unsigned integers exceed the range of a number"),
            DataType::Float16 | DataType::Float32 | DataType::Float64 => Ok(DataType::Float64),
            // A decimal only survives the trip through a 64-bit float if it is
            // narrow enough to be exact there.
            DataType::Decimal128(precision, _) | DataType::Decimal256(precision, _) => {
                if *precision <= MAX_EXACT_DECIMAL_DIGITS {
                    Ok(DataType::Float64)
                } else {
                    Err("a decimal this wide has no exact 64-bit form")
                }
            }
            _ => Err("not a numeric column"),
        },
        ValueType::String => match source {
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => Ok(DataType::Utf8),
            // A true parquet ENUM decodes as binary but holds UTF-8.
            DataType::Binary | DataType::LargeBinary | DataType::BinaryView => Ok(DataType::Utf8),
            _ => Err("not a string column"),
        },
        ValueType::Bool => match source {
            DataType::Boolean => Ok(DataType::Boolean),
            _ => Err("not a boolean column"),
        },
        ValueType::Date => match source {
            DataType::Date32 | DataType::Date64 => Ok(DataType::Date32),
            _ => Err("not a date column"),
        },
        ValueType::Datetime => match source {
            DataType::Timestamp(_, zone) => {
                Ok(DataType::Timestamp(TimeUnit::Microsecond, zone.clone()))
            }
            _ => Err("not a datetime column"),
        },
    }
}

/// Digits a `f64` represents exactly. Beyond this a decimal column's values
/// would be rounded on the way in, so arithmetic over them wouldn't mean what
/// the expression says.
const MAX_EXACT_DECIMAL_DIGITS: u8 = 15;

/// Read the requested columns, cast to the requested types, batch by batch.
///
/// Only the named columns are read. A request [`decodable`] rejects makes this
/// fail rather than silently skipping it — check first.
pub fn read_typed(path: &Path, requests: &[TypedColumnRequest]) -> Result<TypedScan, ParquetError> {
    let ctx = FileContext::open(path)?;
    let mut plans = Vec::with_capacity(requests.len());
    let mut leaves = Vec::with_capacity(requests.len());
    for request in requests {
        let source = ctx.arrow_type_path(&request.path).ok_or_else(|| {
            ParquetError::General(format!(
                "column `{}` cannot be read as a value",
                request.path.join(".")
            ))
        })?;
        let target = decode_target(&source, request.ty).map_err(|why| {
            ParquetError::General(format!("column `{}`: {why}", request.path.join(".")))
        })?;
        let leaf = ctx.leaf_path(&request.path).ok_or_else(|| {
            ParquetError::General(format!("column `{}` is not in the file", request.path[0]))
        })?;
        leaves.push(leaf);
        plans.push(Plan {
            path: request.path.clone(),
            target,
        });
    }
    let reader = ctx.reader(leaves)?;
    Ok(TypedScan {
        reader,
        plans,
        first_row: 0,
    })
}

struct Plan {
    path: Vec<String>,
    target: DataType,
}

/// A streaming read: each item is one batch of the requested columns.
pub struct TypedScan {
    reader: ParquetRecordBatchReader,
    plans: Vec<Plan>,
    first_row: usize,
}

impl Iterator for TypedScan {
    type Item = Result<Batch, ParquetError>;

    fn next(&mut self) -> Option<Self::Item> {
        let batch = match self.reader.next()? {
            Ok(batch) => batch,
            Err(e) => return Some(Err(e.into())),
        };
        let first_row = self.first_row;
        self.first_row += batch.num_rows();
        Some(build(&self.plans, &batch, first_row))
    }
}

fn build(plans: &[Plan], batch: &RecordBatch, first_row: usize) -> Result<Batch, ParquetError> {
    let mut columns = Vec::with_capacity(plans.len());
    for plan in plans {
        let array = batch
            .column_by_name(&plan.path[0])
            .ok_or_else(|| ParquetError::General(format!("column `{}` vanished", plan.path[0])))?;
        let mut current: ArrayRef = Arc::clone(array);
        for segment in &plan.path[1..] {
            let struct_array = current
                .as_struct_opt()
                .ok_or_else(|| ParquetError::General(format!("`{segment}` is not a field")))?;
            current = Arc::clone(
                struct_array
                    .column_by_name(segment)
                    .ok_or_else(|| ParquetError::General(format!("no field `{segment}`")))?,
            );
        }
        // An unrepresentable value is an error rather than a silent null: the
        // caller was told this column was readable.
        let options = CastOptions {
            safe: false,
            ..Default::default()
        };
        columns.push(cast_with_options(&current, &plan.target, &options)?);
    }
    Ok(Batch { first_row, columns })
}

/// One batch of decoded columns, in the order they were requested.
pub struct Batch {
    first_row: usize,
    columns: Vec<ArrayRef>,
}

impl Batch {
    /// The 0-based index of this batch's first row within the file, so a caller
    /// can report an absolute row number.
    pub fn first_row(&self) -> usize {
        self.first_row
    }

    pub fn rows(&self) -> usize {
        self.columns.first().map_or(0, |c| c.len())
    }

    /// The `i`th requested column.
    pub fn column(&self, i: usize) -> Column<'_> {
        let array = &self.columns[i];
        Column {
            validity: array.nulls(),
            values: values_of(array.as_ref()),
        }
    }
}

fn values_of(array: &dyn Array) -> Values<'_> {
    match array.data_type() {
        DataType::Int64 => Values::Int(array.as_primitive::<Int64Type>().values()),
        DataType::Float64 => Values::Float(array.as_primitive::<Float64Type>().values()),
        DataType::Boolean => Values::Bool(BoolValues(array.as_boolean())),
        DataType::Utf8 => Values::Str(StrValues(array.as_string::<i32>())),
        DataType::Date32 => Values::Date(array.as_primitive::<Date32Type>().values()),
        DataType::Timestamp(TimeUnit::Microsecond, zone) => Values::Datetime {
            micros: array.as_primitive::<TimestampMicrosecondType>().values(),
            utc: zone.is_some(),
        },
        // `decode_target` produces nothing else.
        other => unreachable!("undecodable target type {other:?}"),
    }
}

/// A decoded column: its values, and which rows have one.
pub struct Column<'a> {
    values: Values<'a>,
    validity: Option<&'a NullBuffer>,
}

impl<'a> Column<'a> {
    pub fn values(&self) -> &Values<'a> {
        &self.values
    }

    /// Whether row `i` holds a value rather than a null. Rows are indexed
    /// within the batch.
    pub fn is_valid(&self, i: usize) -> bool {
        self.validity.is_none_or(|v| v.is_valid(i))
    }

    /// Whether the column has any nulls at all, so a caller can take a faster
    /// path when it doesn't.
    pub fn all_valid(&self) -> bool {
        self.validity.is_none_or(|v| v.null_count() == 0)
    }
}

/// A column's values, one variant per [`ValueType`]. The numeric variants split
/// by representation: whether a `number` column arrives as integers or floats
/// is a property of the data, which is why the expression language's single
/// `number` type reaches here as two.
pub enum Values<'a> {
    Int(&'a [i64]),
    Float(&'a [f64]),
    Bool(BoolValues<'a>),
    Str(StrValues<'a>),
    /// Days since 1970-01-01.
    Date(&'a [i32]),
    /// Microseconds since 1970-01-01T00:00:00. `utc` distinguishes an instant
    /// from a zoneless local time, which decides whether it can be compared
    /// with the current time.
    Datetime {
        micros: &'a [i64],
        utc: bool,
    },
}

/// Booleans are a bit buffer, so they are read one at a time rather than as a
/// slice.
pub struct BoolValues<'a>(&'a arrow_array::BooleanArray);

impl BoolValues<'_> {
    pub fn get(&self, i: usize) -> bool {
        self.0.value(i)
    }
}

/// Strings live in one buffer behind offsets, so they too are read per row.
pub struct StrValues<'a>(&'a arrow_array::StringArray);

impl<'a> StrValues<'a> {
    pub fn get(&self, i: usize) -> &'a str {
        self.0.value(i)
    }
}
