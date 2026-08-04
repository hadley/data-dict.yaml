//! Uniqueness benchmarks over a generated Parquet fixture.
//!
//! Row count is configurable with `DDP_BENCH_ROWS` (default 3,000,000). The
//! fixture is written once to the temp dir and reused across runs.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use data_dict_parquet::{UniquenessCheck, uniqueness_stats};
use parquet::basic::{Compression, Repetition, Type as PhysicalType};
use parquet::data_type::{ByteArray, ByteArrayType, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::types::Type;

const ROWS_PER_GROUP: usize = 1_000_000;

fn rows() -> usize {
    std::env::var("DDP_BENCH_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3_000_000)
}

fn fixture(rows: usize) -> PathBuf {
    let path = std::env::temp_dir().join(format!("ddp_bench_uniqueness_{rows}.parquet"));
    if path.exists() {
        return path;
    }
    let schema = Arc::new(
        Type::group_type_builder("schema")
            .with_fields(vec![
                Arc::new(field("id", PhysicalType::INT64)),
                Arc::new(field("code", PhysicalType::BYTE_ARRAY)),
                Arc::new(field("part_a", PhysicalType::BYTE_ARRAY)),
                Arc::new(field("part_b", PhysicalType::BYTE_ARRAY)),
            ])
            .build()
            .unwrap(),
    );
    let props = Arc::new(
        WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .build(),
    );
    let mut writer =
        SerializedFileWriter::new(File::create(&path).unwrap(), schema, props).unwrap();
    let mut written = 0;
    while written < rows {
        let n = ROWS_PER_GROUP.min(rows - written);
        let ids: Vec<i64> = (0..n).map(|i| (written + i) as i64).collect();
        let code: Vec<ByteArray> = ids
            .iter()
            .map(|&i| ByteArray::from(format!("CODE-{i:08}").into_bytes()))
            .collect();
        let part_a: Vec<ByteArray> = ids
            .iter()
            .map(|&i| ByteArray::from(format!("G{:04}", i % 1000).into_bytes()))
            .collect();
        let part_b: Vec<ByteArray> = ids
            .iter()
            .map(|&i| ByteArray::from(format!("R{i:08}").into_bytes()))
            .collect();
        let mut group = writer.next_row_group().unwrap();
        let mut col = group.next_column().unwrap().unwrap();
        col.typed::<Int64Type>()
            .write_batch(&ids, None, None)
            .unwrap();
        col.close().unwrap();
        for values in [&code, &part_a, &part_b] {
            let mut col = group.next_column().unwrap().unwrap();
            col.typed::<ByteArrayType>()
                .write_batch(values, None, None)
                .unwrap();
            col.close().unwrap();
        }
        group.close().unwrap();
        written += n;
    }
    writer.close().unwrap();
    path
}

fn field(name: &str, physical: PhysicalType) -> Type {
    Type::primitive_type_builder(name, physical)
        .with_repetition(Repetition::REQUIRED)
        .build()
        .unwrap()
}

fn one(columns: &[&str]) -> Vec<UniquenessCheck> {
    vec![UniquenessCheck {
        columns: columns.iter().map(|s| s.to_string()).collect(),
    }]
}

fn bench(c: &mut Criterion) {
    let path = fixture(rows());
    let run = |checks: &[UniquenessCheck]| uniqueness_stats(Path::new(&path), checks, 5).unwrap();

    let mut group = c.benchmark_group("uniqueness");
    group
        .sample_size(10)
        .warm_up_time(Duration::from_millis(500));
    group.bench_function("id", |b| b.iter(|| run(&one(&["id"]))));
    group.bench_function("code", |b| b.iter(|| run(&one(&["code"]))));
    group.bench_function("pk", |b| b.iter(|| run(&one(&["part_a", "part_b"]))));
    group.bench_function("all", |b| {
        b.iter(|| {
            run(&[
                one(&["id"]).remove(0),
                one(&["code"]).remove(0),
                one(&["part_a", "part_b"]).remove(0),
            ])
        })
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
