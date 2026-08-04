//! Foreign-key (D05) benchmarks over generated parent/child Parquet fixtures.
//!
//! Row count is configurable with `DDP_BENCH_ROWS` (default 3,000,000). Both
//! fixtures hold the same `rows` distinct keys, and every child row references a
//! key that exists — the all-valid case, which is the check's worst case since
//! no probe can short-circuit. `int` exercises the scalar (`i64`) key path,
//! `string` the byte-key path. Fixtures are written once to the temp dir and
//! reused across runs.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use data_dict_parquet::{ForeignKeyCheck, foreign_key_stats};
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

/// A two-column table (`id`, `code`) of `rows` distinct keys, `id[i] == i`. The
/// parent and child fixtures share this shape, so a child key always resolves.
fn fixture(name: &str, rows: usize) -> PathBuf {
    let path = std::env::temp_dir().join(format!("ddp_bench_fk_{name}_{rows}.parquet"));
    if path.exists() {
        return path;
    }
    let schema = Arc::new(
        Type::group_type_builder("schema")
            .with_fields(vec![
                Arc::new(field("id", PhysicalType::INT64)),
                Arc::new(field("code", PhysicalType::BYTE_ARRAY)),
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
        let mut group = writer.next_row_group().unwrap();
        let mut col = group.next_column().unwrap().unwrap();
        col.typed::<Int64Type>()
            .write_batch(&ids, None, None)
            .unwrap();
        col.close().unwrap();
        let mut col = group.next_column().unwrap().unwrap();
        col.typed::<ByteArrayType>()
            .write_batch(&code, None, None)
            .unwrap();
        col.close().unwrap();
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

fn check(child: &Path, parent: &Path, column: &str) -> Vec<ForeignKeyCheck> {
    vec![ForeignKeyCheck {
        child_path: child.to_path_buf(),
        child_column: column.to_string(),
        parent_path: parent.to_path_buf(),
        parent_column: column.to_string(),
    }]
}

fn bench(c: &mut Criterion) {
    let n = rows();
    let parent = fixture("parent", n);
    let child = fixture("child", n);
    let run = |checks: &[ForeignKeyCheck]| foreign_key_stats(checks, 5).unwrap();

    let mut group = c.benchmark_group("foreign_key");
    group
        .sample_size(10)
        .warm_up_time(Duration::from_millis(500));
    group.bench_function("int", |b| b.iter(|| run(&check(&child, &parent, "id"))));
    group.bench_function("string", |b| {
        b.iter(|| run(&check(&child, &parent, "code")))
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
