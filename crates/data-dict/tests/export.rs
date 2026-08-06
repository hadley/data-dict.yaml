//! Integration tests for the export document (`data_dict::export_spec` /
//! `data_dict::export_data`); see `site/export.md` for the shape.

mod common;
use common::{diagnostic, temp_dir, write_dict, write_nested_parquet};

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use data_dict::{Status, export_data, export_spec};
use indoc::indoc;
use parquet::data_type::{ByteArray, ByteArrayType, DoubleType, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;

/// A dictionary exercising every resolved element: dictionary metadata and
/// version, descriptive keys at each level, implied constraints, foreign-key
/// references both ways, struct fields, enum values (map form), an open-ended
/// range, assertions (including a `COLUMNS(...)` expansion), a normalized
/// `one-to-many` relationship, and a glossary.
const RICH_DICT: &str = indoc! {r#"
    $version: "0.1.0"
    $learn_more: http://data-dict.tidyverse.org/
    name: pets
    label: Pet shop
    description: Pets and their owners.
    origin: https://example.com/pets-pipeline
    version:
      number: 1.2.3
    tables:
      - name: owners
        description: One row per owner.
        columns:
          - name: owner_id
            type: number(id)
            constraints: [primary_key]
            examples: [1, 2]
          - name: address
            type: struct
            fields:
              - name: zip
                description: Postal code.
                type: string
                examples: ["97201"]
              - name: tags
                type: list(enum)
                values: [a, b]
      - name: pets
        origin: https://example.com/pets.R
        source:
          parquet: pets.parquet
        columns:
          - name: pet_id
            type: number(id)
            constraints: [primary_key]
            examples: [10]
          - name: owner_id
            label: Owner
            type: number(id)
            constraints: [foreign_key, required]
            examples: [1]
          - name: species
            type: enum
            values:
              cat: A cat
              dog: A dog
          - name: weight
            display: restricted
            type: number(quantity)
            units: kg
            range: [0, 100]
            constraints:
              - assert: weight > 0
                description: Weights are positive.
          - name: born
            type: date
            range: [2000-01-01, .inf]
        constraints:
          - assert: COLUMNS('_id$') IS NOT NULL
    relationships:
      - description: Each pet has one owner.
        cardinality: one-to-many
        join: owners.owner_id = pets.owner_id
    glossary:
      pet: An animal kept for companionship.
      owner: The human a pet keeps.
"#};

/// Export a dictionary body written to a temp dir, requiring success, and
/// return the pretty JSON.
fn export_json(yaml: &str) -> String {
    let path = common::write_yaml(&temp_dir(), yaml);
    let (problems, export) = export_spec(&path);
    let export = export.unwrap_or_else(|| {
        panic!(
            "export should succeed, got:\n{}",
            problems.render(common::SNAPSHOT_STYLE).join("\n")
        )
    });
    serde_json::to_string_pretty(&export).unwrap()
}

#[test]
fn export_spec_resolves_the_dictionary() {
    insta::assert_snapshot!(export_json(RICH_DICT));
}

/// A column with no declared `type` makes no claims and is omitted, including
/// from a `COLUMNS(...)` expansion.
#[test]
fn untyped_columns_are_omitted() {
    let json: serde_json::Value = serde_json::from_str(&export_json(indoc! {r#"
        $version: "0.1.0"
        $learn_more: http://data-dict.tidyverse.org/
        tables:
          - name: animals
            columns:
              - name: id
                type: number(id)
                examples: [1]
              - name: scratch
    "#}))
    .unwrap();
    let names: Vec<&str> = json["tables"][0]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|col| col["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["id"]);
}

/// Every join conjunct exports as a left/right column-reference pair,
/// oriented to the normalized sides — whichever way round the conjunct was
/// written, and with a `one-to-many`'s sides swapped. The original
/// cardinality is kept as `declared_cardinality`.
#[test]
fn relationship_pairs_follow_normalized_sides() {
    let json: serde_json::Value = serde_json::from_str(&export_json(indoc! {r#"
        $version: "0.1.0"
        $learn_more: http://data-dict.tidyverse.org/
        tables:
          - name: periods
            columns:
              - name: start
                type: date
                constraints: [unique]
                range: [2000-01-01, 2020-01-01]
              - name: end
                type: date
                range: [2000-01-01, 2020-01-01]
          - name: visits
            columns:
              - name: day
                type: date
                range: [2000-01-01, 2020-01-01]
        relationships:
          - cardinality: one-to-many
            join: periods.start <= visits.day AND visits.day <= periods.end
    "#}))
    .unwrap();
    let rel = &json["relationships"][0];
    assert_eq!(rel["cardinality"], "many-to-one");
    assert_eq!(rel["declared_cardinality"], "one-to-many");
    assert_eq!(
        rel["pairs"],
        serde_json::json!([
            {
                "left": { "table": "visits", "column": "day" },
                "right": { "table": "periods", "column": "start" }
            },
            {
                "left": { "table": "visits", "column": "day" },
                "right": { "table": "periods", "column": "end" }
            }
        ])
    );
}

/// An invalid dictionary exports nothing and reports the same `S##`
/// diagnostics as `validate-spec`.
#[test]
fn export_spec_fails_with_spec_diagnostics() {
    let dir = temp_dir();
    let path = common::write_yaml(&dir, "tables: []\n");
    let (problems, export) = export_spec(&path);
    assert!(export.is_none());
    assert_eq!(problems.status(), Status::Error);
    let diagnostic = diagnostic(&path, &problems.render(common::SNAPSHOT_STYLE).join("\n"));
    diagnostic.assert_contains(&["S18", "$version"]);
    #[cfg(unix)]
    common::assert_snapshot!(diagnostic);
}

/// Write a three-column parquet file — `id` (int64), `name` (string), `weight`
/// (double) — whose profiles are deterministic: `id` gets an integral
/// histogram, `name` frequent values, and `weight` (continuous) no distinct
/// count.
fn write_profiled_parquet(path: &Path) {
    let message = indoc! {"
        message schema {
            REQUIRED INT64 id;
            REQUIRED BYTE_ARRAY name (UTF8);
            OPTIONAL DOUBLE weight;
        }
    "};
    let schema = Arc::new(parse_message_type(message).unwrap());
    let props = Arc::new(WriterProperties::builder().build());
    let file = File::create(path).unwrap();
    let mut writer = SerializedFileWriter::new(file, schema, props).unwrap();
    let mut rg = writer.next_row_group().unwrap();

    let mut col = rg.next_column().unwrap().unwrap();
    col.typed::<Int64Type>()
        .write_batch(&[1, 2, 3], None, None)
        .unwrap();
    col.close().unwrap();

    let mut col = rg.next_column().unwrap().unwrap();
    col.typed::<ByteArrayType>()
        .write_batch(
            &[
                ByteArray::from("otter"),
                ByteArray::from("otter"),
                ByteArray::from("seal"),
            ],
            None,
            None,
        )
        .unwrap();
    col.close().unwrap();

    let mut col = rg.next_column().unwrap().unwrap();
    col.typed::<DoubleType>()
        .write_batch(&[1.0, 4.0], Some(&[1, 0, 1]), None)
        .unwrap();
    col.close().unwrap();

    rg.close().unwrap();
    writer.close().unwrap();
}

#[test]
fn export_data_profiles_each_column() {
    let dir = temp_dir();
    write_profiled_parquet(&dir.join("data.parquet"));
    let dict = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: animals
                source:
                  parquet: data.parquet
                columns:
                  - name: id
                    type: number(id)
                    constraints: [primary_key]
                    examples: [1]
                  - name: name
                    type: string
                    examples: [otter]
                  - name: weight
                    type: number(quantity)
                    units: kg
                    range: [0, 10]
        "},
    );
    let (problems, export) = export_data(&dict);
    assert_eq!(problems.status(), Status::Ok);
    insta::assert_snapshot!(serde_json::to_string_pretty(&export.unwrap()).unwrap());
}

#[test]
fn export_data_profiles_struct_fields_and_list_containers() {
    let dir = temp_dir();
    write_nested_parquet(&dir.join("data.parquet"));
    let dict = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: animals
                source:
                  parquet: data.parquet
                columns:
                  - name: name
                    type: string
                    examples: [otter]
                  - name: addr
                    type: struct
                    fields:
                      - name: zip
                        type: string
                        examples: ['97201']
                      - name: country
                        type: string
                        examples: [US]
                  - name: tags
                    type: list(string)
                    examples: [a]
        "},
    );
    let (problems, export) = export_data(&dict);
    assert_eq!(problems.status(), Status::Ok);
    let json = serde_json::to_value(export.unwrap()).unwrap();
    assert_eq!(json["tables"][0]["rows"], 3);
    let columns = &json["tables"][0]["columns"];

    // A struct column carries no profile of its own; its fields do.
    assert_eq!(columns[1]["name"], "addr");
    assert!(columns[1]["profile"].is_null());
    let zip = &columns[1]["fields"][0];
    assert_eq!(zip["name"], "zip");
    assert_eq!(zip["profile"]["missing"], 1);
    assert_eq!(zip["profile"]["distinct"]["count"], 2);
    assert_eq!(zip["profile"]["distinct"]["approximate"], false);

    // A list column's profile describes the containers, not the elements.
    assert_eq!(columns[2]["name"], "tags");
    assert_eq!(columns[2]["profile"]["missing"], 1);
    assert!(columns[2]["profile"]["distinct"].is_null());
    assert!(columns[2]["profile"]["histogram"].is_null());
}

/// A table without a `source` is reported as a warning and exported without
/// profiles, rather than failing the export as `validate-meta` would.
#[test]
fn export_data_missing_source_warns_and_still_exports() {
    let dir = temp_dir();
    let dict = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: animals
                columns:
                  - name: id
                    type: number(id)
                    examples: [1]
        "},
    );
    let (problems, export) = export_data(&dict);
    assert_eq!(problems.status(), Status::Warning);
    let export = export.expect("a sourceless table still exports");
    let json = serde_json::to_value(export).unwrap();
    assert!(json["tables"][0]["rows"].is_null());
    assert!(json["tables"][0]["columns"][0]["profile"].is_null());
    let diagnostic = diagnostic(&dict, &problems.render(common::SNAPSHOT_STYLE).join("\n"));
    diagnostic.assert_contains(&["M04", "has no `source`"]);
    #[cfg(unix)]
    common::assert_snapshot!(diagnostic);
}

/// Data that doesn't match the dictionary fails the export with the same
/// diagnostics as `validate-meta`/`validate-data`.
#[test]
fn export_data_fails_on_metadata_mismatch() {
    let dir = temp_dir();
    write_profiled_parquet(&dir.join("data.parquet"));
    let dict = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: animals
                source:
                  parquet: data.parquet
                columns:
                  - name: id
                    type: number(id)
                    examples: [1]
                  - name: name
                    type: string
                    examples: [otter]
                  - name: weight
                    type: string
                    examples: [heavy]
        "},
    );
    let (problems, export) = export_data(&dict);
    assert!(export.is_none());
    assert_eq!(problems.status(), Status::Error);
    let diagnostic = diagnostic(&dict, &problems.render(common::SNAPSHOT_STYLE).join("\n"));
    diagnostic.assert_contains(&["M01", "the data is `number`"]);
    #[cfg(unix)]
    common::assert_snapshot!(diagnostic);
}
