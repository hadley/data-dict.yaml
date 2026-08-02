//! Value canonicalization and hashable key forms, shared by the uniqueness
//! (D02) and foreign-key (D05) checks. See the "comparable types" section of
//! `site/validation.md` for the equality rules implemented here.

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Date64Type, Decimal128Type, Decimal256Type, Float16Type, Float32Type, Float64Type,
    Int8Type, Int16Type, Int32Type, Int64Type, Time32MillisecondType, Time32SecondType,
    Time64MicrosecondType, Time64NanosecondType, TimestampMicrosecondType,
    TimestampMillisecondType, TimestampNanosecondType, TimestampSecondType, UInt8Type, UInt16Type,
    UInt32Type, UInt64Type,
};
use arrow_array::{Array, ArrayRef, BinaryArray, FixedSizeBinaryArray, StringArray};
use arrow_schema::{DataType, TimeUnit};
use hashbrown::{DefaultHashBuilder, HashSet, HashTable};
use std::borrow::Cow;
use std::hash::BuildHasher;
use std::sync::Arc;

use crate::ParquetError;

/// Collapse `-0.0`/`+0.0` to one value and every NaN bit pattern to one value,
/// so logically-equal floats compare equal. 16-bit floats are widened to
/// `f32` (exact), which also gives them the collapsing. Non-float arrays are
/// returned as-is.
///
/// Must run *before* any byte-level key encoding (`arrow-row` and float bits
/// both distinguish zero signs and NaN payloads).
pub(crate) fn canonicalize(array: &ArrayRef) -> ArrayRef {
    match array.data_type() {
        DataType::Float16 => {
            let canon = array
                .as_primitive::<Float16Type>()
                .unary::<_, Float32Type>(|v| canon_float(v.to_f32()));
            Arc::new(canon)
        }
        DataType::Float32 => {
            let canon = array
                .as_primitive::<Float32Type>()
                .unary::<_, Float32Type>(canon_float);
            Arc::new(canon)
        }
        DataType::Float64 => {
            let canon = array
                .as_primitive::<Float64Type>()
                .unary::<_, Float64Type>(canon_double);
            Arc::new(canon)
        }
        _ => array.clone(),
    }
}

fn canon_float(value: f32) -> f32 {
    if value == 0.0 {
        0.0
    } else if value.is_nan() {
        f32::NAN
    } else {
        value
    }
}

fn canon_double(value: f64) -> f64 {
    if value == 0.0 {
        0.0
    } else if value.is_nan() {
        f64::NAN
    } else {
        value
    }
}

/// One (canonicalized) column's values in a hashable form: a scalar per row
/// (borrowed straight from the decoded buffer when the native type is already
/// 8 bytes, materialized otherwise), or a view into the decoded byte values.
pub(crate) enum KeyColumn<'a> {
    /// Values identified by an `i64` — integers, canonicalized float bits,
    /// booleans, dates, times, and timestamps. Junk at null positions.
    I64(Cow<'a, [i64]>),
    /// Decimal values identified by their `i128` representation.
    I128(Cow<'a, [i128]>),
    /// Wide decimals identified by their 32-byte representation.
    Wide(Vec<[u8; 32]>),
    Str(&'a StringArray),
    Bin(&'a BinaryArray),
    Fixed(&'a FixedSizeBinaryArray),
}

impl<'a> KeyColumn<'a> {
    /// View a canonicalized array as key material. Errors on a type the
    /// comparability rules should have barred from reaching a value check.
    pub(crate) fn new(array: &'a ArrayRef) -> Result<KeyColumn<'a>, ParquetError> {
        macro_rules! borrowed {
            ($t:ty) => {
                KeyColumn::I64(Cow::Borrowed(array.as_primitive::<$t>().values()))
            };
        }
        macro_rules! widened {
            ($t:ty, $map:expr) => {
                KeyColumn::I64(Cow::Owned(
                    array
                        .as_primitive::<$t>()
                        .values()
                        .iter()
                        .map(|v| $map(*v))
                        .collect(),
                ))
            };
        }
        Ok(match array.data_type() {
            DataType::Boolean => {
                let bools = array.as_boolean();
                KeyColumn::I64(Cow::Owned(
                    (0..bools.len()).map(|i| bools.value(i) as i64).collect(),
                ))
            }
            DataType::Int8 => widened!(Int8Type, |v: i8| v as i64),
            DataType::Int16 => widened!(Int16Type, |v: i16| v as i64),
            DataType::Int32 => widened!(Int32Type, |v: i32| v as i64),
            DataType::Int64 => borrowed!(Int64Type),
            DataType::UInt8 => widened!(UInt8Type, |v: u8| v as i64),
            DataType::UInt16 => widened!(UInt16Type, |v: u16| v as i64),
            DataType::UInt32 => widened!(UInt32Type, |v: u32| v as i64),
            DataType::UInt64 => widened!(UInt64Type, |v: u64| v as i64),
            DataType::Float32 => widened!(Float32Type, |v: f32| v.to_bits() as i64),
            DataType::Float64 => widened!(Float64Type, |v: f64| v.to_bits() as i64),
            DataType::Date32 => widened!(Date32Type, |v: i32| v as i64),
            DataType::Date64 => borrowed!(Date64Type),
            DataType::Time32(TimeUnit::Second) => widened!(Time32SecondType, |v: i32| v as i64),
            DataType::Time32(TimeUnit::Millisecond) => {
                widened!(Time32MillisecondType, |v: i32| v as i64)
            }
            DataType::Time64(TimeUnit::Microsecond) => borrowed!(Time64MicrosecondType),
            DataType::Time64(TimeUnit::Nanosecond) => borrowed!(Time64NanosecondType),
            DataType::Timestamp(TimeUnit::Second, _) => borrowed!(TimestampSecondType),
            DataType::Timestamp(TimeUnit::Millisecond, _) => borrowed!(TimestampMillisecondType),
            DataType::Timestamp(TimeUnit::Microsecond, _) => borrowed!(TimestampMicrosecondType),
            DataType::Timestamp(TimeUnit::Nanosecond, _) => borrowed!(TimestampNanosecondType),
            DataType::Decimal128(_, _) => KeyColumn::I128(Cow::Borrowed(
                array.as_primitive::<Decimal128Type>().values(),
            )),
            DataType::Decimal256(_, _) => KeyColumn::Wide(
                array
                    .as_primitive::<Decimal256Type>()
                    .values()
                    .iter()
                    .map(|v| v.to_le_bytes())
                    .collect(),
            ),
            DataType::Utf8 => KeyColumn::Str(array.as_string::<i32>()),
            DataType::Binary => KeyColumn::Bin(array.as_binary::<i32>()),
            DataType::FixedSizeBinary(_) => KeyColumn::Fixed(array.as_fixed_size_binary()),
            other => {
                return Err(ParquetError::General(format!(
                    "Cannot compare values of type {other}"
                )));
            }
        })
    }
}

/// A set of already-seen keys matching one [`KeyColumn`] shape, with the same
/// fast paths as before the arrow port: scalars hash bare integers, byte
/// values hash their bytes directly.
pub(crate) enum KeySet {
    I64(HashSet<i64>),
    I128(HashSet<i128>),
    Bytes(ByteKeys),
}

impl KeySet {
    /// An empty set shaped for `column`. `rows` sizes it for one entry per row
    /// up front, skipping incremental rehashes and their transient 2x spike.
    pub(crate) fn for_column(column: &KeyColumn<'_>, rows: usize) -> KeySet {
        match column {
            KeyColumn::I64(_) => KeySet::I64(HashSet::with_capacity(rows)),
            KeyColumn::I128(_) => KeySet::I128(HashSet::with_capacity(rows)),
            _ => KeySet::Bytes(ByteKeys::with_capacity(rows)),
        }
    }

    /// Insert the key at `row`, returning `true` if it was new.
    pub(crate) fn insert(&mut self, column: &KeyColumn<'_>, row: usize) -> bool {
        match (self, column) {
            (KeySet::I64(set), KeyColumn::I64(values)) => set.insert(values[row]),
            (KeySet::I128(set), KeyColumn::I128(values)) => set.insert(values[row]),
            (KeySet::Bytes(set), column) => set.insert(byte_key(column, row)),
            _ => unreachable!("key set shaped for a different column"),
        }
    }

    /// Whether the key at `row` is present, without inserting it.
    pub(crate) fn contains(&self, column: &KeyColumn<'_>, row: usize) -> bool {
        match (self, column) {
            (KeySet::I64(set), KeyColumn::I64(values)) => set.contains(&values[row]),
            (KeySet::I128(set), KeyColumn::I128(values)) => set.contains(&values[row]),
            (KeySet::Bytes(set), column) => set.contains(byte_key(column, row)),
            _ => unreachable!("key set shaped for a different column"),
        }
    }
}

fn byte_key<'a>(column: &'a KeyColumn<'_>, row: usize) -> &'a [u8] {
    match column {
        KeyColumn::Wide(values) => &values[row],
        KeyColumn::Str(array) => array.value(row).as_bytes(),
        KeyColumn::Bin(array) => array.value(row),
        KeyColumn::Fixed(array) => array.value(row),
        KeyColumn::I64(_) | KeyColumn::I128(_) => {
            unreachable!("scalar column keyed as bytes")
        }
    }
}

/// A set of byte-string keys packed into one arena so distinct keys cost an
/// amortised append, not an allocation each. Entries reference the arena by
/// `(chunk, offset, length)`, and a single hash probe both tests membership and,
/// when absent, positions the insertion.
#[derive(Default)]
pub(crate) struct ByteKeys {
    arena: Arena,
    table: HashTable<KeyRef>,
    hasher: DefaultHashBuilder,
}

/// A key's location in the arena: `(chunk, offset within chunk, length)`.
type KeyRef = (u32, u32, u32);

impl ByteKeys {
    pub(crate) fn with_capacity(rows: usize) -> Self {
        ByteKeys {
            arena: Arena::default(),
            table: HashTable::with_capacity(rows),
            hasher: DefaultHashBuilder::default(),
        }
    }

    /// Insert `key`, returning `true` if it was new (i.e. not already present).
    pub(crate) fn insert(&mut self, key: &[u8]) -> bool {
        let Self {
            arena,
            table,
            hasher,
        } = self;
        let hash = hasher.hash_one(key);
        if table.find(hash, |&r| arena.get(r) == key).is_some() {
            return false;
        }
        let entry = arena.push(key);
        table.insert_unique(hash, entry, |&r| hasher.hash_one(arena.get(r)));
        true
    }

    /// Whether `key` is present, without inserting it.
    pub(crate) fn contains(&self, key: &[u8]) -> bool {
        let hash = self.hasher.hash_one(key);
        self.table
            .find(hash, |&r| self.arena.get(r) == key)
            .is_some()
    }
}

/// Append-only byte store backed by fixed-size chunks. Existing chunks are never
/// reallocated, so growth is incremental — there is no transient doubling spike
/// as with one growing `Vec`, and no need to estimate the total size up front.
#[derive(Default)]
struct Arena {
    chunks: Vec<Vec<u8>>,
}

impl Arena {
    const CHUNK: usize = 4 * 1024 * 1024;

    fn push(&mut self, key: &[u8]) -> KeyRef {
        let fits = self
            .chunks
            .last()
            .is_some_and(|chunk| chunk.capacity() - chunk.len() >= key.len());
        if !fits {
            self.chunks
                .push(Vec::with_capacity(key.len().max(Self::CHUNK)));
        }
        let chunk = self.chunks.len() as u32 - 1;
        let buffer = self.chunks.last_mut().unwrap();
        let offset = buffer.len() as u32;
        buffer.extend_from_slice(key);
        (chunk, offset, key.len() as u32)
    }

    fn get(&self, (chunk, offset, len): KeyRef) -> &[u8] {
        &self.chunks[chunk as usize][offset as usize..offset as usize + len as usize]
    }
}
