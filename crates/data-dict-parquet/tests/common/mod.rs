//! Parquet fixtures for the profiling tests.
//!
//! Each [`Fixture`] writes one small file to a unique temp path. The writer
//! settings that matter here are the ones that change which code path the
//! profiler takes: dictionary encoding on or off, and footer statistics present
//! or absent (which decides whether histogram bins need a second pass).
#![allow(dead_code)]

use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use parquet::data_type::{
    BoolType, ByteArray, ByteArrayType, DoubleType, FloatType, Int32Type, Int64Type,
};
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::file::writer::{SerializedColumnWriter, SerializedFileWriter};
use parquet::schema::parser::parse_message_type;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// One column's values for one row group, `None` being a null.
#[derive(Clone)]
pub enum Values {
    Int32(Vec<Option<i32>>),
    Int64(Vec<Option<i64>>),
    Float(Vec<Option<f32>>),
    Double(Vec<Option<f64>>),
    Bool(Vec<Option<bool>>),
    Text(Vec<Option<String>>),
    /// Raw byte arrays, for writing values that aren't valid UTF-8.
    Bytes(Vec<Option<Vec<u8>>>),
}

impl Values {
    pub fn int64(values: impl IntoIterator<Item = i64>) -> Values {
        Values::Int64(values.into_iter().map(Some).collect())
    }

    pub fn int32(values: impl IntoIterator<Item = i32>) -> Values {
        Values::Int32(values.into_iter().map(Some).collect())
    }

    pub fn double(values: impl IntoIterator<Item = f64>) -> Values {
        Values::Double(values.into_iter().map(Some).collect())
    }

    pub fn text<'a>(values: impl IntoIterator<Item = &'a str>) -> Values {
        Values::Text(values.into_iter().map(|v| Some(v.to_string())).collect())
    }

    pub fn bool(values: impl IntoIterator<Item = bool>) -> Values {
        Values::Bool(values.into_iter().map(Some).collect())
    }
}

pub struct Fixture {
    fields: Vec<String>,
    groups: Vec<Vec<Values>>,
    dictionary: bool,
    statistics: bool,
    small_pages: bool,
    version: WriterVersion,
}

impl Fixture {
    /// A file whose schema is `fields`, each a Parquet schema line such as
    /// `"OPTIONAL INT64 v"`.
    pub fn new(fields: &[&str]) -> Self {
        Fixture {
            fields: fields.iter().map(|f| f.to_string()).collect(),
            groups: Vec::new(),
            dictionary: true,
            statistics: true,
            small_pages: false,
            version: WriterVersion::PARQUET_1_0,
        }
    }

    /// A one-column file holding `values` in a single row group.
    pub fn column(field: &str, values: Values) -> Self {
        Fixture::new(&[field]).group(vec![values])
    }

    pub fn group(mut self, values: Vec<Values>) -> Self {
        self.groups.push(values);
        self
    }

    pub fn dictionary(mut self, enabled: bool) -> Self {
        self.dictionary = enabled;
        self
    }

    pub fn statistics(mut self, enabled: bool) -> Self {
        self.statistics = enabled;
        self
    }

    /// Cap the dictionary and data pages at a few bytes, so a column chunk
    /// spills into several pages and abandons the dictionary partway through.
    pub fn small_pages(mut self) -> Self {
        self.small_pages = true;
        self
    }

    /// Write v2 data pages, which state their null count in the page header
    /// rather than leaving it to be counted from the definition levels.
    pub fn version(mut self, version: WriterVersion) -> Self {
        self.version = version;
        self
    }

    pub fn write(self) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ddp-profile-{}-{}.parquet",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let message = format!("message schema {{ {}; }}", self.fields.join("; "));
        let schema = Arc::new(parse_message_type(&message).unwrap());
        let optional: Vec<bool> = self
            .fields
            .iter()
            .map(|field| field.contains("OPTIONAL"))
            .collect();

        let mut properties = WriterProperties::builder()
            .set_writer_version(self.version)
            .set_dictionary_enabled(self.dictionary)
            .set_statistics_enabled(if self.statistics {
                EnabledStatistics::Chunk
            } else {
                EnabledStatistics::None
            });
        if self.small_pages {
            properties = properties
                .set_dictionary_page_size_limit(64)
                .set_data_page_size_limit(64)
                .set_write_batch_size(16);
        }

        let file = File::create(&path).unwrap();
        let mut writer =
            SerializedFileWriter::new(file, schema, Arc::new(properties.build())).unwrap();
        for group in &self.groups {
            let mut row_group = writer.next_row_group().unwrap();
            for (values, optional) in group.iter().zip(&optional) {
                let mut column = row_group.next_column().unwrap().unwrap();
                write_values(&mut column, values, *optional);
                column.close().unwrap();
            }
            row_group.close().unwrap();
        }
        writer.close().unwrap();
        path
    }
}

/// A file whose only column sits inside a group, so it has no top-level
/// primitive to profile.
pub fn write_nested() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ddp-profile-nested-{}-{}.parquet",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let schema = Arc::new(
        parse_message_type("message schema { OPTIONAL group g { REQUIRED INT64 x; } }").unwrap(),
    );
    let file = File::create(&path).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::new())).unwrap();
    let mut row_group = writer.next_row_group().unwrap();
    let mut column = row_group.next_column().unwrap().unwrap();
    column
        .typed::<Int64Type>()
        .write_batch(&[1, 2], Some(&[1, 1]), None)
        .unwrap();
    column.close().unwrap();
    row_group.close().unwrap();
    writer.close().unwrap();
    path
}

fn write_values(column: &mut SerializedColumnWriter, values: &Values, optional: bool) {
    /// Write the present values with definition levels marking the nulls.
    macro_rules! write {
        ($type:ty, $items:expr, $map:expr) => {{
            let levels: Vec<i16> = $items.iter().map(|v| v.is_some() as i16).collect();
            let present: Vec<_> = $items.iter().flatten().map($map).collect();
            let levels = optional.then_some(levels.as_slice());
            column
                .typed::<$type>()
                .write_batch(&present, levels, None)
                .unwrap();
        }};
    }
    match values {
        Values::Int32(items) => write!(Int32Type, items, |v| *v),
        Values::Int64(items) => write!(Int64Type, items, |v| *v),
        Values::Float(items) => write!(FloatType, items, |v| *v),
        Values::Double(items) => write!(DoubleType, items, |v| *v),
        Values::Bool(items) => write!(BoolType, items, |v| *v),
        Values::Text(items) => write!(ByteArrayType, items, |v| ByteArray::from(v.as_str())),
        Values::Bytes(items) => write!(ByteArrayType, items, |v| ByteArray::from(v.clone())),
    }
}
