//! Parquet reader for data-dict.yaml validation.

mod dictionary;
mod display;
mod foreign_key;
mod keys;
mod metadata;
mod page;
mod profile;
mod reader;
mod scan;
mod sketch;
mod uniqueness;
mod value;

pub use foreign_key::{ForeignKeyCheck, ForeignKeyResult, ForeignKeyStats, foreign_key_stats};
pub use metadata::{
    ColumnMeta, ColumnTypeInfo, column_meta, column_type_info, column_types, uniqueness_barriers,
};
pub use parquet::errors::ParquetError;
pub use profile::{Bin, ColumnProfile, Distinct, FileProfile, Histogram, NotFinite, profile};
pub use scan::{ColumnNeeds, ColumnStats, column_stats};
pub use sketch::ValueCount;
pub use uniqueness::{UniquenessCheck, UniquenessStats, uniqueness_stats};
pub use value::{F64, TimeGrain, Value, ValueKind};
