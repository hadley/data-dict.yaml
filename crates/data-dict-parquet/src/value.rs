//! The typed values the profiler summarizes columns in.
//!
//! Profiling counts, orders and hashes values, so [`Value`] covers exactly the
//! types that support all three. Every other Parquet type classifies as
//! [`ValueKind::Unsupported`] and is profiled down to row and null counts.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Int8Type, Int16Type, Int32Type, Int64Type, Time32MillisecondType,
    Time64MicrosecondType, Time64NanosecondType, TimestampMicrosecondType,
    TimestampMillisecondType, TimestampNanosecondType, TimestampSecondType, UInt8Type, UInt16Type,
    UInt32Type,
};
use arrow_array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float32Array, Float64Array, StringArray,
};
use arrow_schema::DataType;
use parquet::basic::{LogicalType, Repetition, TimeUnit, Type as PhysicalType};
use parquet::schema::types::Type;

use crate::ParquetError;
use crate::page::for_each_plain_byte_array;

/// One column value, canonicalized so logically-equal values compare equal.
///
/// Dates, times and timestamps are their raw physical numbers; a column's
/// [`ValueKind`] says how to read them. All of one column's values share a
/// single variant, so the derived cross-variant ordering never comes into play.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(F64),
    Text(String),
}

impl Value {
    /// The value's position on a numeric scale, for histogram binning. `None`
    /// for kinds that have no such scale.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(value) => Some(*value as f64),
            Value::Float(value) => Some(value.get()),
            Value::Bool(_) | Value::Text(_) => None,
        }
    }
}

/// A finite `f64` with a total order, so floats can key a hash map and sort.
/// `-0.0` is canonicalized to `0.0` on construction, matching how the value
/// scanners hash floats (see the "comparable types" section of
/// `site/validation.md`).
#[derive(Debug, Clone, Copy)]
pub struct F64(f64);

impl F64 {
    /// `None` unless `value` is finite. NaN is not equal to itself, and an
    /// infinity has no position on the number line — as a minimum or a maximum
    /// it would stretch a histogram's bins to infinite width. The profiler
    /// counts both apart from the values (see [`Histogram`]).
    ///
    /// This is what guarantees every [`Value::Float`] is finite, so anything
    /// derived from one — bin edges above all — is finite too.
    ///
    /// [`Histogram`]: crate::Histogram
    pub fn new(value: f64) -> Option<F64> {
        value
            .is_finite()
            .then_some(F64(if value == 0.0 { 0.0 } else { value }))
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl PartialEq for F64 {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for F64 {}

impl PartialOrd for F64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for F64 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Hash for F64 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeGrain {
    Millis,
    Micros,
    Nanos,
}

/// What a column's values mean, beyond the [`Value`] variant that carries them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueKind {
    Bool,
    Int,
    Float,
    Text,
    /// Days since the Unix epoch.
    Date,
    /// Time of day since midnight, in `grain` units.
    Time {
        grain: TimeGrain,
    },
    /// Time since the Unix epoch in `grain` units. `utc_adjusted` is Parquet's
    /// `isAdjustedToUTC`: true means the instant is UTC, false means it carries
    /// no zone at all.
    Timestamp {
        grain: TimeGrain,
        utc_adjusted: bool,
    },
    /// A type the profiler doesn't summarize, with a short slug naming the
    /// barrier (as `Comparability::Incomparable` does for uniqueness).
    Unsupported(&'static str),
}

impl ValueKind {
    /// Whether the values have a meaningful order, so a minimum, a maximum, and
    /// examples spread along the sorted values all mean something.
    pub fn is_ordered(&self) -> bool {
        !matches!(self, ValueKind::Bool | ValueKind::Unsupported(_))
    }

    /// Whether the values sit on a numeric scale that splits into equal-width
    /// bins. Text is ordered but has no such scale.
    pub fn is_binnable(&self) -> bool {
        matches!(
            self,
            ValueKind::Int
                | ValueKind::Float
                | ValueKind::Date
                | ValueKind::Time { .. }
                | ValueKind::Timestamp { .. }
        )
    }
}

/// How a column's raw dictionary-page values become [`Value`]s. Kept apart
/// from [`ValueKind`] because it answers a decoding question, not a meaning
/// one. The arrow value scan doesn't need it — decoded arrays carry their own
/// types (see [`ValueColumn`]) — but raw dictionary pages hold bare physical
/// values, so their decoding is steered here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Repr {
    Bool,
    Int,
    /// An unsigned integer narrower than 64 bits, which a raw page stores in a
    /// signed physical type and so must be masked back to `bits` when read
    /// from a dictionary page. (Arrow-decoded arrays arrive unsigned already.)
    Uint(u32),
    Float32,
    Float64,
    Text,
    Unsupported,
}

/// The outcome of turning one raw value into a [`Value`].
pub(crate) enum Decoded {
    Value(Value),
    /// A NaN or an infinity, which is neither null nor a value with a place on
    /// the number line, so it is counted apart. Carries the float itself, which
    /// is all the caller needs to tell the three cases apart.
    NotFinite(f64),
    /// A byte array that isn't UTF-8, which downgrades its whole column to
    /// [`ValueKind::Unsupported`]. Arises only for unannotated `BYTE_ARRAY`
    /// columns (and dictionary pages): a column *declared* `STRING` that holds
    /// invalid UTF-8 fails arrow's decode validation as a read error instead.
    NotUtf8,
}

/// Classify a column: what its values mean, and how to decode them.
pub(crate) fn classify(field: &Type) -> (ValueKind, Repr) {
    let info = field.get_basic_info();
    // A repeated field holds a list per row, so its values don't line up with
    // rows and a per-value summary would describe something else.
    if !field.is_primitive() || (info.has_repetition() && info.repetition() == Repetition::REPEATED)
    {
        return unsupported("nested");
    }
    if let Some(logical) = field.get_basic_info().logical_type() {
        return match logical {
            LogicalType::String | LogicalType::Enum => (ValueKind::Text, Repr::Text),
            LogicalType::Date => (ValueKind::Date, Repr::Int),
            LogicalType::Time { unit, .. } => (ValueKind::Time { grain: grain(unit) }, Repr::Int),
            LogicalType::Timestamp {
                unit,
                is_adjusted_to_u_t_c,
            } => (
                ValueKind::Timestamp {
                    grain: grain(unit),
                    utc_adjusted: is_adjusted_to_u_t_c,
                },
                Repr::Int,
            ),
            LogicalType::Integer {
                bit_width,
                is_signed,
            } => integer(bit_width, is_signed),
            // Fitting a decimal into `Value` would mean either losing the scale
            // or losing exactness, and neither is worth it for a summary.
            LogicalType::Decimal { .. } => unsupported("decimal"),
            LogicalType::Uuid => unsupported("uuid"),
            // Could now be profiled (arrow decodes half-floats as floats), but
            // stays out of the summary until someone wants it.
            LogicalType::Float16 => unsupported("float16"),
            // Equal documents can differ byte for byte, so counting distinct
            // encodings would be misleading.
            LogicalType::Json => unsupported("json"),
            LogicalType::Bson => unsupported("bson"),
            LogicalType::Map | LogicalType::List => unsupported("nested"),
            LogicalType::Unknown => unsupported("unknown"),
        };
    }
    match field.get_physical_type() {
        PhysicalType::BOOLEAN => (ValueKind::Bool, Repr::Bool),
        PhysicalType::INT32 | PhysicalType::INT64 => (ValueKind::Int, Repr::Int),
        PhysicalType::FLOAT => (ValueKind::Float, Repr::Float32),
        PhysicalType::DOUBLE => (ValueKind::Float, Repr::Float64),
        PhysicalType::BYTE_ARRAY => (ValueKind::Text, Repr::Text),
        PhysicalType::FIXED_LEN_BYTE_ARRAY => unsupported("binary"),
        PhysicalType::INT96 => unsupported("int96"),
    }
}

fn integer(bit_width: i8, is_signed: bool) -> (ValueKind, Repr) {
    if is_signed {
        return (ValueKind::Int, Repr::Int);
    }
    match bit_width {
        // `u64` values above `i64::MAX` have no home in `Value::Int`, and
        // silently wrapping them would put the minimum above the maximum.
        64 => unsupported("uint64"),
        bits => (ValueKind::Int, Repr::Uint(bits as u32)),
    }
}

fn unsupported(reason: &'static str) -> (ValueKind, Repr) {
    (ValueKind::Unsupported(reason), Repr::Unsupported)
}

fn grain(unit: TimeUnit) -> TimeGrain {
    match unit {
        TimeUnit::MILLIS(_) => TimeGrain::Millis,
        TimeUnit::MICROS(_) => TimeGrain::Micros,
        TimeUnit::NANOS(_) => TimeGrain::Nanos,
    }
}

/// One decoded arrow batch column, viewed as profile values.
///
/// Integer-meaning kinds (including dates, times and timestamps, whose raw
/// numbers are what [`Value`] carries) collapse to one `i64` slice — borrowed
/// straight from the decoded buffer when the native type is already 8 bytes.
/// Narrow unsigned integers arrive from arrow as unsigned arrays, so no
/// sign-extension masking is needed here (unlike the raw dictionary path).
pub(crate) enum ValueColumn<'a> {
    Bool(&'a BooleanArray),
    Int(Cow<'a, [i64]>),
    Float32(&'a Float32Array),
    Float64(&'a Float64Array),
    Str(&'a StringArray),
    Bin(&'a BinaryArray),
}

impl<'a> ValueColumn<'a> {
    /// View a decoded array as profile values. Errors on a type `classify`
    /// should have barred from being scanned.
    pub(crate) fn new(array: &'a ArrayRef) -> Result<ValueColumn<'a>, ParquetError> {
        use arrow_schema::TimeUnit::*;
        macro_rules! borrowed {
            ($t:ty) => {
                ValueColumn::Int(Cow::Borrowed(array.as_primitive::<$t>().values()))
            };
        }
        macro_rules! widened {
            ($t:ty) => {
                ValueColumn::Int(Cow::Owned(
                    array
                        .as_primitive::<$t>()
                        .values()
                        .iter()
                        .map(|v| *v as i64)
                        .collect(),
                ))
            };
        }
        Ok(match array.data_type() {
            DataType::Boolean => ValueColumn::Bool(array.as_boolean()),
            DataType::Int8 => widened!(Int8Type),
            DataType::Int16 => widened!(Int16Type),
            DataType::Int32 => widened!(Int32Type),
            DataType::Int64 => borrowed!(Int64Type),
            DataType::UInt8 => widened!(UInt8Type),
            DataType::UInt16 => widened!(UInt16Type),
            DataType::UInt32 => widened!(UInt32Type),
            DataType::Date32 => widened!(Date32Type),
            DataType::Time32(Millisecond) => widened!(Time32MillisecondType),
            DataType::Time64(Microsecond) => borrowed!(Time64MicrosecondType),
            DataType::Time64(Nanosecond) => borrowed!(Time64NanosecondType),
            DataType::Timestamp(Second, _) => borrowed!(TimestampSecondType),
            DataType::Timestamp(Millisecond, _) => borrowed!(TimestampMillisecondType),
            DataType::Timestamp(Microsecond, _) => borrowed!(TimestampMicrosecondType),
            DataType::Timestamp(Nanosecond, _) => borrowed!(TimestampNanosecondType),
            DataType::Float32 => ValueColumn::Float32(array.as_primitive()),
            DataType::Float64 => ValueColumn::Float64(array.as_primitive()),
            DataType::Utf8 => ValueColumn::Str(array.as_string::<i32>()),
            // A parquet ENUM decodes as binary; its values are still UTF-8.
            DataType::Binary => ValueColumn::Bin(array.as_binary::<i32>()),
            other => {
                return Err(ParquetError::General(format!(
                    "Cannot profile values of type {other}"
                )));
            }
        })
    }

    /// The value at `row`, which must not be null.
    pub(crate) fn get(&self, row: usize) -> Decoded {
        match self {
            ValueColumn::Bool(array) => Decoded::Value(Value::Bool(array.value(row))),
            ValueColumn::Int(values) => Decoded::Value(Value::Int(values[row])),
            ValueColumn::Float32(array) => float(array.value(row) as f64),
            ValueColumn::Float64(array) => float(array.value(row)),
            ValueColumn::Str(array) => Decoded::Value(Value::Text(array.value(row).to_string())),
            ValueColumn::Bin(array) => text(array.value(row)),
        }
    }
}

/// Decode a PLAIN-encoded dictionary page into its values, in index order.
/// `None` for a malformed buffer or a physical type that is never dictionary
/// encoded, leaving the caller to fall back to the value scan.
pub(crate) fn decode_dictionary(
    buf: &[u8],
    count: usize,
    physical: PhysicalType,
    repr: Repr,
) -> Option<Vec<Decoded>> {
    match physical {
        PhysicalType::BYTE_ARRAY => decode_byte_arrays(buf, count),
        PhysicalType::INT32 => decode_fixed::<4>(buf, count, |b| {
            integer_value(i32::from_le_bytes(b) as i64, repr)
        }),
        PhysicalType::INT64 => {
            decode_fixed::<8>(buf, count, |b| integer_value(i64::from_le_bytes(b), repr))
        }
        PhysicalType::FLOAT => {
            decode_fixed::<4>(buf, count, |b| float(f32::from_le_bytes(b) as f64))
        }
        PhysicalType::DOUBLE => decode_fixed::<8>(buf, count, |b| float(f64::from_le_bytes(b))),
        _ => None,
    }
}

/// Decode `count` PLAIN byte arrays. Every one is kept, valid UTF-8 or not —
/// which of the two it is decides the whole column's fate, not this value's.
fn decode_byte_arrays(buf: &[u8], count: usize) -> Option<Vec<Decoded>> {
    let mut values = Vec::with_capacity(count);
    let complete = for_each_plain_byte_array(buf, count, |value| {
        values.push(text(value));
        true
    });
    complete.then_some(values)
}

fn decode_fixed<const N: usize>(
    buf: &[u8],
    count: usize,
    decode: impl Fn([u8; N]) -> Decoded,
) -> Option<Vec<Decoded>> {
    if buf.len() < count * N {
        return None;
    }
    Some(
        buf.chunks_exact(N)
            .take(count)
            .map(|chunk| decode(chunk.try_into().unwrap()))
            .collect(),
    )
}

fn integer_value(raw: i64, repr: Repr) -> Decoded {
    match repr {
        Repr::Uint(bits) => Decoded::Value(Value::Int(unsign(raw, bits))),
        _ => Decoded::Value(Value::Int(raw)),
    }
}

fn unsign(raw: i64, bits: u32) -> i64 {
    raw & ((1i64 << bits) - 1)
}

fn float(value: f64) -> Decoded {
    match F64::new(value) {
        Some(finite) => Decoded::Value(Value::Float(finite)),
        None => Decoded::NotFinite(value),
    }
}

fn text(bytes: &[u8]) -> Decoded {
    match std::str::from_utf8(bytes) {
        Ok(text) => Decoded::Value(Value::Text(text.to_string())),
        Err(_) => Decoded::NotUtf8,
    }
}

#[cfg(test)]
mod tests {
    use super::{F64, Repr, TimeGrain, Value, ValueKind, classify};
    use parquet::schema::parser::parse_message_type;

    fn kind_of(field_line: &str) -> (ValueKind, Repr) {
        let message = format!("message schema {{ {field_line}; }}");
        let schema = parse_message_type(&message).unwrap();
        classify(&schema.get_fields()[0])
    }

    #[test]
    fn scalar_types_are_classified() {
        assert_eq!(
            kind_of("REQUIRED BYTE_ARRAY s (STRING)"),
            (ValueKind::Text, Repr::Text)
        );
        assert_eq!(kind_of("REQUIRED BOOLEAN b"), (ValueKind::Bool, Repr::Bool));
        assert_eq!(kind_of("REQUIRED INT64 i"), (ValueKind::Int, Repr::Int));
        assert_eq!(
            kind_of("REQUIRED FLOAT f"),
            (ValueKind::Float, Repr::Float32)
        );
        assert_eq!(
            kind_of("REQUIRED DOUBLE d"),
            (ValueKind::Float, Repr::Float64)
        );
        assert_eq!(
            kind_of("REQUIRED INT32 d (DATE)"),
            (ValueKind::Date, Repr::Int)
        );
    }

    #[test]
    fn timestamps_carry_grain_and_utc_adjustment() {
        assert_eq!(
            kind_of("REQUIRED INT64 t (TIMESTAMP(MICROS,true))").0,
            ValueKind::Timestamp {
                grain: TimeGrain::Micros,
                utc_adjusted: true
            }
        );
        assert_eq!(
            kind_of("REQUIRED INT64 t (TIMESTAMP(MILLIS,false))").0,
            ValueKind::Timestamp {
                grain: TimeGrain::Millis,
                utc_adjusted: false
            }
        );
        assert_eq!(
            kind_of("REQUIRED INT32 t (TIME(MILLIS,true))").0,
            ValueKind::Time {
                grain: TimeGrain::Millis
            }
        );
    }

    #[test]
    fn narrow_unsigned_integers_are_masked_but_u64_is_not_profiled() {
        assert_eq!(
            kind_of("REQUIRED INT32 u (INTEGER(32,false))"),
            (ValueKind::Int, Repr::Uint(32))
        );
        assert_eq!(
            kind_of("REQUIRED INT64 u (INTEGER(64,false))").0,
            ValueKind::Unsupported("uint64")
        );
        assert_eq!(
            kind_of("REQUIRED INT64 i (INTEGER(64,true))"),
            (ValueKind::Int, Repr::Int)
        );
    }

    #[test]
    fn unprofilable_types_report_their_barrier() {
        for (line, reason) in [
            ("REQUIRED INT64 dec (DECIMAL(9,2))", "decimal"),
            ("REQUIRED FIXED_LEN_BYTE_ARRAY(16) uu (UUID)", "uuid"),
            ("REQUIRED FIXED_LEN_BYTE_ARRAY(4) raw", "binary"),
            ("REQUIRED INT96 t", "int96"),
            ("REQUIRED BYTE_ARRAY j (JSON)", "json"),
            ("REQUIRED BYTE_ARRAY b (BSON)", "bson"),
        ] {
            let (kind, repr) = kind_of(line);
            assert_eq!(kind, ValueKind::Unsupported(reason), "{line}");
            assert_eq!(repr, Repr::Unsupported, "{line}");
        }
    }

    #[test]
    fn only_ordered_kinds_have_min_max_and_only_numeric_ones_bin() {
        assert!(ValueKind::Date.is_ordered() && ValueKind::Date.is_binnable());
        assert!(ValueKind::Text.is_ordered() && !ValueKind::Text.is_binnable());
        assert!(!ValueKind::Bool.is_ordered() && !ValueKind::Bool.is_binnable());
        assert!(!ValueKind::Unsupported("nested").is_ordered());
    }

    #[test]
    fn signed_zeros_collapse_and_only_finite_floats_are_values() {
        assert_eq!(F64::new(-0.0), F64::new(0.0));
        assert!(F64::new(-1.0) < F64::new(1.0));
        assert_eq!(Value::Int(7).as_f64(), Some(7.0));
        assert_eq!(Value::Text("x".into()).as_f64(), None);

        // Nothing that would make a bin edge non-finite can become a value.
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(F64::new(value).is_none(), "{value} must not be a value");
        }
    }
}
