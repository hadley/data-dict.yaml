//! Files written by arrow embed an arrow schema whose types (LargeUtf8,
//! Dictionary, …) differ from what the parquet schema implies. The reader must
//! ignore it — arrow is only our decoder — so these columns validate exactly
//! like ones from any other writer.

use std::fs::File;
use std::sync::Arc;

use arrow_array::types::Int32Type;
use arrow_array::{ArrayRef, DictionaryArray, LargeStringArray, RecordBatch};
use data_dict_parquet::{UniquenessCheck, uniqueness_stats};
use parquet::arrow::ArrowWriter;

#[test]
fn arrow_written_file_ignores_embedded_schema() {
    let large: ArrayRef = Arc::new(LargeStringArray::from(vec!["a", "b", "a"]));
    let dict: DictionaryArray<Int32Type> = ["x", "y", "x"].into_iter().collect();
    let batch =
        RecordBatch::try_from_iter([("large", large), ("dict", Arc::new(dict) as ArrayRef)])
            .unwrap();

    let dir = std::env::temp_dir().join(format!("ddp_arrow_written_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("data.parquet");
    let mut writer =
        ArrowWriter::try_new(File::create(&path).unwrap(), batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    for column in ["large", "dict"] {
        let checks = [UniquenessCheck {
            columns: vec![column.to_string()],
        }];
        let stats = uniqueness_stats(&path, &checks, 5).unwrap();
        assert_eq!(stats[0].duplicate_count, 1, "column {column}");
        assert_eq!(stats[0].duplicate_rows, vec![3], "column {column}");
    }
}
