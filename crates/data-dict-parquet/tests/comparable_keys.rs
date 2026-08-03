//! Every type `uniqueness_comparability` classifies as comparable must
//! actually be checkable: the decoded arrow array has to key without error,
//! and an equal pair has to register as a duplicate. This pins the bridge
//! between the parquet-schema classification and the arrow-side key forms.

use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use data_dict_parquet::{UniquenessCheck, uniqueness_stats};
use parquet::data_type::{
    BoolType, ByteArray, ByteArrayType, DoubleType, FixedLenByteArray, FixedLenByteArrayType,
    FloatType, Int32Type, Int64Type, Int96, Int96Type,
};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;

/// Comparable schema shapes, with the FIXED_LEN_BYTE_ARRAY width where one
/// applies. Two rows holding the same value are written for each.
const COMPARABLE: &[(&str, usize)] = &[
    ("REQUIRED BOOLEAN v", 0),
    ("REQUIRED INT32 v", 0),
    ("REQUIRED INT64 v", 0),
    ("REQUIRED INT96 v", 0),
    ("REQUIRED FLOAT v", 0),
    ("REQUIRED DOUBLE v", 0),
    ("REQUIRED BYTE_ARRAY v", 0),
    ("REQUIRED BYTE_ARRAY v (STRING)", 0),
    ("REQUIRED BYTE_ARRAY v (ENUM)", 0),
    ("REQUIRED BYTE_ARRAY v (DECIMAL(9,2))", 0),
    ("REQUIRED INT32 v (INTEGER(8,true))", 0),
    ("REQUIRED INT32 v (INTEGER(16,false))", 0),
    ("REQUIRED INT64 v (INTEGER(64,false))", 0),
    ("REQUIRED INT32 v (DATE)", 0),
    ("REQUIRED INT32 v (TIME_MILLIS)", 0),
    ("REQUIRED INT64 v (TIME_MICROS)", 0),
    ("REQUIRED INT64 v (TIMESTAMP_MILLIS)", 0),
    ("REQUIRED INT64 v (TIMESTAMP_MICROS)", 0),
    ("REQUIRED INT64 v (TIMESTAMP(NANOS,true))", 0),
    ("REQUIRED INT32 v (DECIMAL(9,2))", 0),
    ("REQUIRED INT64 v (DECIMAL(18,2))", 0),
    ("REQUIRED FIXED_LEN_BYTE_ARRAY(3) v", 3),
    ("REQUIRED FIXED_LEN_BYTE_ARRAY(16) v (UUID)", 16),
    ("REQUIRED FIXED_LEN_BYTE_ARRAY(2) v (FLOAT16)", 2),
    ("REQUIRED FIXED_LEN_BYTE_ARRAY(16) v (DECIMAL(38,10))", 16),
    ("REQUIRED FIXED_LEN_BYTE_ARRAY(32) v (DECIMAL(76,10))", 32),
];

#[test]
fn every_comparable_type_is_keyable() {
    let dir = std::env::temp_dir().join(format!("ddp_comparable_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (index, (line, flba_len)) in COMPARABLE.iter().enumerate() {
        let path = dir.join(format!("{index}.parquet"));
        write_pair(&path, line, *flba_len);
        let checks = [UniquenessCheck {
            columns: vec!["v".to_string()],
        }];
        let stats = uniqueness_stats(&path, &checks, 5)
            .unwrap_or_else(|e| panic!("uniqueness failed for `{line}`: {e}"));
        assert_eq!(
            stats[0].duplicate_count, 1,
            "no duplicate seen for `{line}`"
        );
        assert_eq!(stats[0].duplicate_rows, vec![2], "wrong row for `{line}`");
    }
}

/// Write a one-column file with the same value in both rows.
fn write_pair(path: &PathBuf, schema_line: &str, flba_len: usize) {
    let message = format!("message schema {{ {schema_line}; }}");
    let schema = Arc::new(parse_message_type(&message).unwrap());
    let physical = schema.get_fields()[0].get_physical_type();
    let file = File::create(path).unwrap();
    let mut writer =
        SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build()))
            .unwrap();
    let mut rg = writer.next_row_group().unwrap();
    let mut col = rg.next_column().unwrap().unwrap();
    use parquet::basic::Type as PhysicalType;
    match physical {
        PhysicalType::BOOLEAN => {
            col.typed::<BoolType>()
                .write_batch(&[true, true], None, None)
                .unwrap();
        }
        PhysicalType::INT32 => {
            col.typed::<Int32Type>()
                .write_batch(&[7, 7], None, None)
                .unwrap();
        }
        PhysicalType::INT64 => {
            col.typed::<Int64Type>()
                .write_batch(&[7, 7], None, None)
                .unwrap();
        }
        PhysicalType::INT96 => {
            let v = Int96::from(vec![1, 2, 3]);
            col.typed::<Int96Type>()
                .write_batch(&[v, v], None, None)
                .unwrap();
        }
        PhysicalType::FLOAT => {
            col.typed::<FloatType>()
                .write_batch(&[1.5, 1.5], None, None)
                .unwrap();
        }
        PhysicalType::DOUBLE => {
            col.typed::<DoubleType>()
                .write_batch(&[1.5, 1.5], None, None)
                .unwrap();
        }
        PhysicalType::BYTE_ARRAY => {
            let v = ByteArray::from(vec![0x01_u8]);
            col.typed::<ByteArrayType>()
                .write_batch(&[v.clone(), v], None, None)
                .unwrap();
        }
        PhysicalType::FIXED_LEN_BYTE_ARRAY => {
            let v = FixedLenByteArray::from(vec![0x01_u8; flba_len]);
            col.typed::<FixedLenByteArrayType>()
                .write_batch(&[v.clone(), v], None, None)
                .unwrap();
        }
    }
    col.close().unwrap();
    rg.close().unwrap();
    writer.close().unwrap();
}
