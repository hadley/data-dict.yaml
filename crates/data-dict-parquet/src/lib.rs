//! Parquet reader for data-dict.yaml validation.

mod describe;
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

pub use describe::{FileDescription, Scalar, describe, edge_scalar, render_scalar};
pub use foreign_key::{ForeignKeyCheck, ForeignKeyResult, ForeignKeyStats, foreign_key_stats};
pub use metadata::{
    ColumnMeta, ColumnTypeInfo, DataColumn, column_meta, column_tree, column_type_info, row_count,
    uniqueness_barriers,
};
pub use parquet::errors::ParquetError;
pub use profile::{
    Bin, ColumnProfile, DEFAULT_MAX_BINS, Distinct, FileProfile, Histogram, NotFinite, profile,
    profile_paths,
};
pub use scan::{ColumnNeeds, ColumnRequest, ColumnStats, column_stats};
pub use sketch::ValueCount;
pub use uniqueness::{UniquenessCheck, UniquenessStats, uniqueness_stats};
pub use value::{F64, TimeGrain, Value, ValueKind, date_iso, datetime_iso, time_iso};
