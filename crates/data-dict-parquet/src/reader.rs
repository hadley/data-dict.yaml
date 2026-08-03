//! Shared arrow-based file access: one footer parse per file, from which every
//! check constructs its own projected, in-order record-batch reader.

use std::fs::File;
use std::path::Path;

use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReader,
    ParquetRecordBatchReaderBuilder,
};
use parquet::file::metadata::ParquetMetaData;

use crate::ParquetError;

/// Rows decoded per record batch. Large enough to amortise per-batch overhead,
/// small enough that a batch of every scanned column stays in cache.
pub(crate) const BATCH_ROWS: usize = 8192;

/// An opened parquet file with its footer parsed once; readers for any column
/// projection are constructed from it without re-reading the metadata.
pub(crate) struct FileContext {
    file: File,
    meta: ArrowReaderMetadata,
}

impl FileContext {
    pub(crate) fn open(path: &Path) -> Result<Self, ParquetError> {
        let file = File::open(path)
            .map_err(|e| ParquetError::General(format!("Cannot open file: {e}")))?;
        // Ignore any embedded arrow schema: it would reproduce writer-side
        // arrow types (LargeUtf8, Dictionary, views, …) where we want the
        // types the *parquet* schema implies — arrow is only our decoder.
        let options = ArrowReaderOptions::new().with_skip_arrow_metadata(true);
        let meta = ArrowReaderMetadata::load(&file, options)?;
        Ok(FileContext { file, meta })
    }

    pub(crate) fn parquet(&self) -> &ParquetMetaData {
        self.meta.metadata()
    }

    pub(crate) fn rows(&self) -> usize {
        self.parquet().file_metadata().num_rows().max(0) as usize
    }

    /// The leaf index of the column named `name`, if present.
    pub(crate) fn leaf(&self, name: &str) -> Option<usize> {
        let descr = self.parquet().file_metadata().schema_descr();
        (0..descr.num_columns()).find(|&i| descr.column(i).name() == name)
    }

    /// The parquet schema descriptor behind a leaf, for comparability
    /// classification.
    pub(crate) fn leaf_descr(&self, leaf: usize) -> parquet::schema::types::ColumnDescPtr {
        self.parquet().file_metadata().schema_descr().column(leaf)
    }

    /// The arrow type a top-level column decodes to.
    pub(crate) fn arrow_type(&self, name: &str) -> Result<arrow_schema::DataType, ParquetError> {
        Ok(self
            .meta
            .schema()
            .field_with_name(name)?
            .data_type()
            .clone())
    }

    /// A fresh handle on the underlying file, for readers that need the
    /// non-arrow API (the D04 dictionary-page fast path).
    pub(crate) fn file(&self) -> Result<File, ParquetError> {
        self.file
            .try_clone()
            .map_err(|e| ParquetError::General(format!("Cannot reopen file: {e}")))
    }

    /// An in-order record-batch reader over just the given leaves.
    pub(crate) fn reader(
        &self,
        leaves: impl IntoIterator<Item = usize>,
    ) -> Result<ParquetRecordBatchReader, ParquetError> {
        self.builder(leaves)?.build()
    }

    /// Like [`FileContext::reader`], restricted to a single row group.
    pub(crate) fn group_reader(
        &self,
        leaves: impl IntoIterator<Item = usize>,
        group: usize,
    ) -> Result<ParquetRecordBatchReader, ParquetError> {
        self.builder(leaves)?.with_row_groups(vec![group]).build()
    }

    /// Like [`FileContext::group_reader`] for a single leaf, but requesting it
    /// as a `Dictionary(Int32, _)` array: a dictionary-encoded chunk decodes
    /// its distinct values once and its rows as indices into them.
    pub(crate) fn dictionary_group_reader(
        &self,
        leaf: usize,
        group: usize,
    ) -> Result<ParquetRecordBatchReader, ParquetError> {
        let name = self.leaf_descr(leaf).name().to_string();
        let schema = self.meta.schema();
        let fields: Vec<arrow_schema::FieldRef> = schema
            .fields()
            .iter()
            .map(|field| {
                if field.name() == &name {
                    let wrapped = arrow_schema::DataType::Dictionary(
                        Box::new(arrow_schema::DataType::Int32),
                        Box::new(field.data_type().clone()),
                    );
                    std::sync::Arc::new(field.as_ref().clone().with_data_type(wrapped))
                } else {
                    field.clone()
                }
            })
            .collect();
        let schema = std::sync::Arc::new(arrow_schema::Schema::new_with_metadata(
            fields,
            schema.metadata().clone(),
        ));
        let options = ArrowReaderOptions::new()
            .with_skip_arrow_metadata(true)
            .with_schema(schema);
        let meta = ArrowReaderMetadata::try_new(self.meta.metadata().clone(), options)?;
        let mask = ProjectionMask::leaves(self.parquet().file_metadata().schema_descr(), [leaf]);
        ParquetRecordBatchReaderBuilder::new_with_metadata(self.file()?, meta)
            .with_projection(mask)
            .with_row_groups(vec![group])
            .with_batch_size(BATCH_ROWS)
            .build()
    }

    fn builder(
        &self,
        leaves: impl IntoIterator<Item = usize>,
    ) -> Result<ParquetRecordBatchReaderBuilder<File>, ParquetError> {
        let mask = ProjectionMask::leaves(self.parquet().file_metadata().schema_descr(), leaves);
        Ok(
            ParquetRecordBatchReaderBuilder::new_with_metadata(self.file()?, self.meta.clone())
                .with_projection(mask)
                .with_batch_size(BATCH_ROWS),
        )
    }
}
