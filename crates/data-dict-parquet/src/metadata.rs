use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

use parquet::basic::{LogicalType, Repetition, TimeUnit, Type as PhysicalType};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::schema::types::Type;

use crate::ParquetError;

pub struct ColumnTypeInfo {
    pub name: String,
    pub dict_type: String,
    pub logical_type: Option<String>,
    pub physical_type: String,
}

/// Footer statistics that can settle data-level checks without reading values.
#[derive(Debug, Clone, Copy)]
pub struct ColumnMeta {
    /// Total nulls across all row groups, or `None` when any row group omits
    /// null-count statistics. Required Parquet fields always report `Some(0)`.
    pub null_count: Option<usize>,
    /// Number of rows in the file.
    pub row_count: usize,
    /// Distinct values when a single row group's footer provides the count.
    /// Multiple row-group counts cannot prove file-wide uniqueness.
    pub distinct_count: Option<usize>,
}

/// Read the inexpensive, footer-only statistics for each top-level column.
///
/// Row-group statistics are per *leaf*, so each top-level field is mapped to
/// its leaf range. A scalar column reads its one leaf directly. A nested
/// column (struct, list) aggregates: its leaves' null counts conflate a null
/// container with a null/absent value further down, so they prove only the
/// all-zero case — every leaf at zero means no definition level ever dropped,
/// hence no null containers. Anything else stays `None` (settled by a scan).
pub fn column_meta(path: &Path) -> Result<HashMap<String, ColumnMeta>, ParquetError> {
    let file =
        File::open(path).map_err(|e| ParquetError::General(format!("Cannot open file: {e}")))?;
    let reader = SerializedFileReader::new(file)?;
    let meta = reader.metadata();
    let fields = meta.file_metadata().schema().get_fields();
    let row_count = meta.file_metadata().num_rows() as usize;

    let leaf_nulls = |leaf: usize| {
        meta.row_groups().iter().try_fold(0usize, |total, rg| {
            rg.column(leaf)
                .statistics()
                .and_then(|s| s.null_count_opt())
                .map(|count| total + count as usize)
        })
    };

    let mut out = HashMap::new();
    let mut leaf = 0usize;
    for field in fields {
        let leaves = leaves_under(field);
        let scalar = field.is_primitive();
        let info = field.get_basic_info();
        let required = info.has_repetition() && info.repetition() == Repetition::REQUIRED;
        let null_count = if required {
            Some(0)
        } else if scalar {
            leaf_nulls(leaf)
        } else {
            (leaf..leaf + leaves)
                .try_fold(0usize, |total, l| leaf_nulls(l).map(|n| total + n))
                .filter(|&total| total == 0)
        };
        let distinct_count = match (scalar, meta.row_groups()) {
            (true, [row_group]) => row_group
                .column(leaf)
                .statistics()
                .and_then(|statistics| statistics.distinct_count_opt())
                .map(|count| count as usize),
            _ => None,
        };
        out.insert(
            field.name().to_string(),
            ColumnMeta {
                null_count,
                row_count,
                distinct_count,
            },
        );
        leaf += leaves;
    }
    Ok(out)
}

/// The number of primitive leaves under `t` (itself, if primitive) — the
/// row-group columns it spans.
fn leaves_under(t: &Type) -> usize {
    if t.is_primitive() {
        1
    } else {
        t.get_fields().iter().map(|f| leaves_under(f)).sum()
    }
}

/// The number of rows in the file, from its footer.
pub fn row_count(path: &Path) -> Result<usize, ParquetError> {
    let file =
        File::open(path).map_err(|e| ParquetError::General(format!("Cannot open file: {e}")))?;
    let reader = SerializedFileReader::new(file)?;
    Ok(reader.metadata().file_metadata().num_rows() as usize)
}

pub fn column_type_info(path: &Path) -> Result<Vec<ColumnTypeInfo>, ParquetError> {
    let file =
        File::open(path).map_err(|e| ParquetError::General(format!("Cannot open file: {e}")))?;
    let reader = SerializedFileReader::new(file)?;
    let schema = reader.metadata().file_metadata().schema();
    Ok(schema
        .get_fields()
        .iter()
        .map(|field| {
            let info = field.get_basic_info();
            ColumnTypeInfo {
                name: field.name().to_string(),
                dict_type: parquet_type_to_dict_type(field),
                logical_type: info.logical_type().map(format_logical_type),
                physical_type: if field.is_primitive() {
                    format!("{:?}", field.get_physical_type())
                } else {
                    "GROUP".to_string()
                },
            }
        })
        .collect())
}

fn format_logical_type(logical_type: LogicalType) -> String {
    match logical_type {
        LogicalType::String => "String".into(),
        LogicalType::Map => "Map".into(),
        LogicalType::List => "List".into(),
        LogicalType::Enum => "Enum".into(),
        LogicalType::Decimal { precision, scale } => format!("Decimal({precision},{scale})"),
        LogicalType::Date => "Date".into(),
        LogicalType::Time {
            is_adjusted_to_u_t_c,
            unit,
        } => {
            let unit = format_time_unit(unit);
            let timezone = if is_adjusted_to_u_t_c { "UTC" } else { "local" };
            format!("Time({unit},{timezone})")
        }
        LogicalType::Timestamp {
            is_adjusted_to_u_t_c,
            unit,
        } => {
            let unit = format_time_unit(unit);
            let timezone = if is_adjusted_to_u_t_c { "UTC" } else { "local" };
            format!("Timestamp({unit},{timezone})")
        }
        LogicalType::Integer {
            bit_width,
            is_signed,
        } => {
            let sign = if is_signed { "i" } else { "u" };
            format!("Integer({sign}{bit_width})")
        }
        LogicalType::Unknown => "Unknown".into(),
        LogicalType::Json => "Json".into(),
        LogicalType::Bson => "Bson".into(),
        LogicalType::Uuid => "Uuid".into(),
        LogicalType::Float16 => "Float16".into(),
    }
}

fn format_time_unit(unit: TimeUnit) -> &'static str {
    match unit {
        TimeUnit::MILLIS(_) => "ms",
        TimeUnit::MICROS(_) => "us",
        TimeUnit::NANOS(_) => "ns",
    }
}

/// Whether a column's values can be compared for the uniqueness checks (D02) —
/// see the "comparable types" section of `site/validation.md`. Arrow decoding
/// settles most representation questions (decimals arrive as numeric values,
/// 16-bit floats as floats, INT96 as timestamps); floats are additionally
/// canonicalized before hashing. `Incomparable` carries a short slug naming the
/// barrier, used to build the D03 warning.
pub(crate) enum Comparability {
    Comparable,
    Incomparable(&'static str),
}

pub(crate) fn uniqueness_comparability(field: &Type) -> Comparability {
    use Comparability::{Comparable, Incomparable};
    let info = field.get_basic_info();
    if !field.is_primitive() || (info.has_repetition() && info.repetition() == Repetition::REPEATED)
    {
        return Incomparable("nested");
    }
    if let Some(logical) = field.get_basic_info().logical_type() {
        return match logical {
            LogicalType::String
            | LogicalType::Enum
            | LogicalType::Date
            | LogicalType::Time { .. }
            | LogicalType::Timestamp { .. }
            | LogicalType::Integer { .. }
            | LogicalType::Decimal { .. }
            | LogicalType::Float16
            | LogicalType::Uuid => Comparable,
            LogicalType::Json => Incomparable("json"),
            LogicalType::Bson => Incomparable("bson"),
            LogicalType::Map | LogicalType::List => Incomparable("nested"),
            LogicalType::Unknown => Incomparable("unknown"),
        };
    }
    match field.get_physical_type() {
        PhysicalType::BOOLEAN
        | PhysicalType::INT32
        | PhysicalType::INT64
        | PhysicalType::INT96
        | PhysicalType::FLOAT
        | PhysicalType::DOUBLE
        | PhysicalType::BYTE_ARRAY
        | PhysicalType::FIXED_LEN_BYTE_ARRAY => Comparable,
    }
}

/// The barrier reason for each top-level column that can't be compared for the
/// uniqueness checks, keyed by column name. Comparable columns are absent.
pub fn uniqueness_barriers(path: &Path) -> Result<HashMap<String, &'static str>, ParquetError> {
    let file =
        File::open(path).map_err(|e| ParquetError::General(format!("Cannot open file: {e}")))?;
    let reader = SerializedFileReader::new(file)?;
    let schema = reader.metadata().file_metadata().schema();
    Ok(schema
        .get_fields()
        .iter()
        .filter_map(|field| match uniqueness_comparability(field) {
            Comparability::Incomparable(reason) => Some((field.name().to_string(), reason)),
            Comparability::Comparable => None,
        })
        .collect())
}

fn parquet_type_to_dict_type(field: &Type) -> String {
    let info = field.get_basic_info();

    // A repeated field with no LIST wrapper is the legacy two-level list
    // encoding: the field itself is the element (see site/validation.md,
    // "Nested Parquet types").
    if info.has_repetition() && info.repetition() == Repetition::REPEATED {
        return format!("list({})", parquet_element_type(field));
    }

    parquet_element_type(field)
}

/// The data-dict type of `field` itself, ignoring its repetition.
fn parquet_element_type(field: &Type) -> String {
    if let Some(logical) = field.get_basic_info().logical_type() {
        match logical {
            LogicalType::String => return "string".into(),
            LogicalType::Enum => return "enum".into(),
            LogicalType::Date => return "date".into(),
            LogicalType::Timestamp { .. } => return "datetime".into(),
            LogicalType::Integer { .. } | LogicalType::Float16 | LogicalType::Decimal { .. } => {
                return "number".into();
            }
            LogicalType::List => {
                let elem = parquet_list_element(field)
                    .map(parquet_element_type)
                    .unwrap_or_else(|| "string".into());
                return format!("list({elem})");
            }
            // A map's keys are data, not schema, so no data-dict type
            // describes one; `map` never matches a declared type (M01).
            LogicalType::Map => return "map".into(),
            _ => {}
        }
    }

    // Group types with no list/map logical annotation are structs.
    if field.is_group() {
        return "struct".into();
    }

    match field.get_physical_type() {
        PhysicalType::BOOLEAN => "boolean".into(),
        PhysicalType::INT32 | PhysicalType::INT64 => "number".into(),
        PhysicalType::INT96 => "datetime".into(),
        PhysicalType::FLOAT | PhysicalType::DOUBLE => "number".into(),
        PhysicalType::BYTE_ARRAY | PhysicalType::FIXED_LEN_BYTE_ARRAY => "string".into(),
    }
}

/// Navigate a LIST-annotated group to its element field. The standard
/// three-level encoding is `group (LIST) { repeated group list { <element> } }`;
/// legacy writers also produced two-level forms where the repeated child is
/// itself the element — a primitive, or a multi-field group (a struct element).
/// Returns `None` when the structure deviates from every known layout.
fn parquet_list_element(field: &Type) -> Option<&Type> {
    let repeated = field.get_fields().first()?;
    if repeated.is_primitive() {
        return Some(repeated);
    }
    match repeated.get_fields() {
        // One child is the standard three-level wrapper — unless the group's
        // own name marks it as a single-field struct element (the format's
        // backward-compatibility rule for legacy writers).
        [element] if repeated.name() != "array" && !repeated.name().ends_with("_tuple") => {
            Some(element)
        }
        _ => Some(repeated),
    }
}

/// The leaf ordinal (row-group column index) reached by `path` — a top-level
/// column name followed by struct field names, with list wrappers crossed
/// transparently, mirroring the dict-type mapping's descent. A path ending on
/// a nested node (a struct, or a list) resolves to its first leaf, enough to
/// project the column for an arrow read.
pub(crate) fn leaf_index(schema: &Type, path: &[String]) -> Option<usize> {
    let mut offset = 0usize;
    let mut node: Option<&Type> = None;
    for field in schema.get_fields() {
        if field.name() == path[0] {
            node = Some(field);
            break;
        }
        offset += leaves_under(field);
    }
    let mut node = node?;
    for segment in &path[1..] {
        // Cross list wrappers so a field of `list(struct)` resolves like a
        // field of `struct`. The wrapper's other leaves (none in practice)
        // precede the element.
        while let Some(element) = list_wrapper_element(node) {
            node = element;
        }
        if node.is_primitive() {
            return None;
        }
        let mut found = None;
        for field in node.get_fields() {
            if field.name() == *segment {
                found = Some(field);
                break;
            }
            offset += leaves_under(field);
        }
        node = found?;
    }
    // Descend to the node's first leaf.
    while !node.is_primitive() {
        node = node.get_fields().first()?;
    }
    Some(offset)
}

/// The element node when `field` is list-shaped (LIST-annotated, or the legacy
/// repeated encoding where the field is its own element), `None` otherwise.
fn list_wrapper_element(field: &Type) -> Option<&Type> {
    let info = field.get_basic_info();
    if matches!(info.logical_type(), Some(LogicalType::List)) {
        return parquet_list_element(field);
    }
    None
}

/// A column (or nested field) as read from the data: its name, its mapped
/// data-dict type, and — when it holds a struct, directly or as list
/// elements — the struct's fields.
#[derive(Debug, Clone)]
pub struct DataColumn {
    pub name: String,
    pub dict_type: String,
    pub children: Vec<DataColumn>,
}

/// Read every top-level column with its mapped data-dict type, descending into
/// `struct` and `list(struct)` columns so metadata validation can check
/// declared fields against the data.
pub fn column_tree(path: &Path) -> Result<Vec<DataColumn>, ParquetError> {
    let file =
        File::open(path).map_err(|e| ParquetError::General(format!("Cannot open file: {e}")))?;
    let reader = SerializedFileReader::new(file)?;
    let schema = reader.metadata().file_metadata().schema();
    Ok(schema.get_fields().iter().map(|f| data_column(f)).collect())
}

fn data_column(field: &Type) -> DataColumn {
    let dict_type = parquet_type_to_dict_type(field);
    let children = struct_group(field, &dict_type)
        .map(|group| group.get_fields().iter().map(|f| data_column(f)).collect())
        .unwrap_or_default();
    DataColumn {
        name: field.name().to_string(),
        dict_type,
        children,
    }
}

/// The group holding this column's struct fields: the column itself for
/// `struct`, the element group for a list of structs — crossing one list
/// layer per `list(...)` wrapper in `dict_type`, however deep. In the legacy
/// repeated encoding the repeated field is its own element, so that layer
/// crosses nowhere.
fn struct_group<'a>(field: &'a Type, dict_type: &str) -> Option<&'a Type> {
    let mut node = field;
    let mut inner = dict_type;
    while let Some(elem) = inner
        .strip_prefix("list(")
        .and_then(|s| s.strip_suffix(")"))
    {
        inner = elem;
        let info = node.get_basic_info();
        let legacy_repeated = info.has_repetition()
            && info.repetition() == Repetition::REPEATED
            && !matches!(info.logical_type(), Some(LogicalType::List));
        if !legacy_repeated {
            node = parquet_list_element(node)?;
        }
    }
    (inner == "struct").then_some(node)
}

#[cfg(test)]
mod tests {
    use super::{Comparability, parquet_type_to_dict_type, uniqueness_comparability};
    use parquet::schema::parser::parse_message_type;

    fn classify(field_line: &str) -> Comparability {
        let message = format!("message schema {{ {field_line}; }}");
        let schema = parse_message_type(&message).unwrap();
        uniqueness_comparability(&schema.get_fields()[0])
    }

    #[test]
    fn nested_fields_map_without_panicking() {
        for (message, expected) in [
            (
                "message schema { OPTIONAL group g { REQUIRED INT64 x; } }",
                "struct",
            ),
            // The legacy two-level encodings: a repeated field is its own element.
            ("message schema { REPEATED INT32 xs; }", "list(number)"),
            (
                "message schema { REPEATED group g { REQUIRED INT64 x; REQUIRED INT64 y; } }",
                "list(struct)",
            ),
        ] {
            let schema = parse_message_type(message).unwrap();
            assert_eq!(parquet_type_to_dict_type(&schema.get_fields()[0]), expected);
        }
    }

    #[test]
    fn list_and_map_annotations_map_to_dict_types() {
        for (message, expected) in [
            (
                "message schema { OPTIONAL group tags (LIST) { REPEATED group list { OPTIONAL BYTE_ARRAY element (STRING); } } }",
                "list(string)",
            ),
            (
                "message schema { OPTIONAL group items (LIST) { REPEATED group list { OPTIONAL group element { REQUIRED INT64 x; } } } }",
                "list(struct)",
            ),
            (
                "message schema { OPTIONAL group m (MAP) { REPEATED group key_value { REQUIRED BYTE_ARRAY key (STRING); OPTIONAL INT32 value; } } }",
                "map",
            ),
            (
                "message schema { OPTIONAL group grid (LIST) { REPEATED group list { OPTIONAL group element (LIST) { REPEATED group list { OPTIONAL BYTE_ARRAY element (STRING); } } } } }",
                "list(list(string))",
            ),
        ] {
            let schema = parse_message_type(message).unwrap();
            assert_eq!(parquet_type_to_dict_type(&schema.get_fields()[0]), expected);
        }
    }

    #[test]
    fn column_tree_descends_through_nested_lists() {
        let message = "message schema {
            OPTIONAL group cells (LIST) {
                REPEATED group list {
                    OPTIONAL group element (LIST) {
                        REPEATED group list {
                            OPTIONAL group element { REQUIRED INT64 qty; }
                        }
                    }
                }
            }
        }";
        let schema = parse_message_type(message).unwrap();
        let cells = super::data_column(&schema.get_fields()[0]);
        assert_eq!(cells.dict_type, "list(list(struct))");
        assert_eq!(cells.children[0].name, "qty");
        assert_eq!(cells.children[0].dict_type, "number");
    }

    #[test]
    fn column_tree_descends_into_structs() {
        let message = "message schema {
            OPTIONAL group addr {
                REQUIRED BYTE_ARRAY zip (STRING);
                OPTIONAL group geo { REQUIRED DOUBLE lat; }
            }
            OPTIONAL group items (LIST) {
                REPEATED group list {
                    OPTIONAL group element { REQUIRED INT64 qty; }
                }
            }
        }";
        let schema = parse_message_type(message).unwrap();
        let addr = super::data_column(&schema.get_fields()[0]);
        assert_eq!(addr.dict_type, "struct");
        assert_eq!(addr.children[0].dict_type, "string");
        assert_eq!(addr.children[1].dict_type, "struct");
        assert_eq!(addr.children[1].children[0].name, "lat");
        let items = super::data_column(&schema.get_fields()[1]);
        assert_eq!(items.dict_type, "list(struct)");
        assert_eq!(items.children[0].name, "qty");
        assert_eq!(items.children[0].dict_type, "number");
    }

    #[test]
    fn comparable_types_are_recognized() {
        for line in [
            "REQUIRED BYTE_ARRAY s (STRING)",
            "REQUIRED BYTE_ARRAY u (UTF8)",
            "REQUIRED INT64 i (INTEGER(64,true))",
            "REQUIRED INT32 d (DATE)",
            "REQUIRED BOOLEAN b",
            "REQUIRED FIXED_LEN_BYTE_ARRAY(16) uu (UUID)",
            "REQUIRED INT64 dec (DECIMAL(9,2))",
            "REQUIRED BYTE_ARRAY dec2 (DECIMAL(9,2))",
            "REQUIRED DOUBLE f",
            "REQUIRED FLOAT g",
            "REQUIRED FIXED_LEN_BYTE_ARRAY(2) h (FLOAT16)",
        ] {
            assert!(
                matches!(classify(line), Comparability::Comparable),
                "expected comparable: {line}"
            );
        }
    }

    #[test]
    fn uncomparable_types_report_their_barrier() {
        for (line, reason) in [
            ("REQUIRED BYTE_ARRAY j (JSON)", "json"),
            ("REQUIRED BYTE_ARRAY b (BSON)", "bson"),
        ] {
            assert!(
                matches!(classify(line), Comparability::Incomparable(r) if r == reason),
                "expected barrier {reason}: {line}"
            );
        }
    }
}
