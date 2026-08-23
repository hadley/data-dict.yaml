//! Integration tests for the export document (`data_dict::export_spec` /
//! `data_dict::export_data` / `data_dict::export_auto`); see `site/export.md`
//! for the shape.

mod common;
use common::{diagnostic, temp_dir, write_dict, write_nested_parquet};

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use data_dict::{Status, export_auto, export_data, export_spec};
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

/// Each assertion carries its translations: one entry per target, with the
/// predicate as `code`. A target that can't express a construct still
/// appears, carrying the refusal as `error` instead — and a translation that
/// diverges on a documented edge says so in `notes`.
#[test]
fn assertions_export_their_translations() {
    let json: serde_json::Value = serde_json::from_str(&export_json(indoc! {r#"
        $version: "0.1.0"
        $learn_more: http://data-dict.tidyverse.org/
        tables:
          - name: people
            columns:
              - name: name
                type: string
                examples: [ada]
              - name: postcode
                type: string
                examples: [SW1A 1AA]
              - name: qty
                type: number(quantity)
                range: [0, .inf]
                constraints:
                  - assert: qty / 2 > 1
            constraints:
              - assert: name LIKE LOWER(postcode)
    "#}))
    .unwrap();
    let columns = &json["tables"][0]["columns"];

    let translations = &columns[2]["assertions"][0]["translations"];
    assert_eq!(translations[0]["target"], "SQL(duckdb)");
    assert_eq!(translations[0]["code"], r#""qty" / 2 > 1"#);
    assert_eq!(translations[1]["target"], "R(tidyverse)");
    assert_eq!(translations[1]["code"], "qty / 2L > 1L");
    // Comparing over numbers diverges on a NaN, and says so.
    assert!(
        translations[0]["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("NaN"))
    );

    // R has no computed LIKE pattern, so it refuses with a reason.
    let table_assertion = &json["tables"][0]["constraints"][0];
    let refused = &table_assertion["translations"][1];
    assert_eq!(refused["target"], "R(tidyverse)");
    assert!(refused.get("code").is_none());
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("computed pattern")
    );
    // DuckDB translates the same assertion fine.
    assert_eq!(
        table_assertion["translations"][0]["code"],
        r#""name" LIKE lower("postcode")"#
    );
}

/// An enum written as a map keeps its labels: the values themselves still list
/// in declaration order, and each one's label is looked up by the value. The
/// list form has nothing to label, so it exports no `value_labels` at all.
#[test]
fn enum_labels_survive_the_map_form() {
    let json: serde_json::Value = serde_json::from_str(&export_json(indoc! {r#"
        $version: "0.1.0"
        $learn_more: http://data-dict.tidyverse.org/
        tables:
          - name: people
            columns:
              - name: sex
                type: enum
                values: {M: Male, F: Female}
              - name: status
                type: enum
                values: [active, closed]
    "#}))
    .unwrap();
    let columns = &json["tables"][0]["columns"];
    assert_eq!(columns[0]["values"], serde_json::json!(["M", "F"]));
    assert_eq!(
        columns[0]["value_labels"],
        serde_json::json!({ "M": "Male", "F": "Female" })
    );
    assert_eq!(
        columns[1]["values"],
        serde_json::json!(["active", "closed"])
    );
    assert!(columns[1].get("value_labels").is_none());
}

/// A `todo` exports wherever the spec allows one — the dataset, a table, a
/// column, a struct field, a relationship — rendered from Markdown like the
/// other prose, so the list form `draft` writes arrives as a list. A level
/// without one exports no `todo` at all.
#[test]
fn todos_export_at_every_level() {
    let json: serde_json::Value = serde_json::from_str(&export_json(indoc! {r#"
        $version: "0.1.0"
        $learn_more: http://data-dict.tidyverse.org/
        todo: Confirm the years covered.
        tables:
          - name: survey
            todo: Check the grain.
            columns:
              - name: survey_id
                type: number(id)
                constraints: [primary_key]
                examples: [1]
              - name: counted
                type: number
                examples: [1]
                todo: |
                  - Add a `description`.
                  - Confirm whether 0 means none seen.
              - name: place
                type: struct
                fields:
                  - name: river
                    type: string
                    examples: [Esk]
                    todo: Is this the river or the reach?
          - name: site
            columns:
              - name: survey_id
                type: number(id)
                constraints: [foreign_key]
                examples: [1]
        relationships:
          - join: site.survey_id = survey.survey_id
            cardinality: many-to-one
            todo: Confirm no sites predate their survey.
    "#}))
    .unwrap();

    assert_eq!(json["todo"], "<p>Confirm the years covered.</p>");
    let survey = &json["tables"][0];
    assert_eq!(survey["todo"], "<p>Check the grain.</p>");
    assert_eq!(
        survey["columns"][1]["todo"],
        "<ul>\n<li>Add a <code>description</code>.</li>\n\
         <li>Confirm whether 0 means none seen.</li>\n</ul>"
    );
    assert_eq!(
        survey["columns"][2]["fields"][0]["todo"],
        "<p>Is this the river or the reach?</p>"
    );
    assert_eq!(
        json["relationships"][0]["todo"],
        "<p>Confirm no sites predate their survey.</p>"
    );

    assert!(survey["columns"][0].get("todo").is_none());
    assert!(json["tables"][1].get("todo").is_none());
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

/// A `display: restricted` column's profile carries counts only — never a
/// value the data held (no samples, common values, observed range, or
/// histogram).
#[test]
fn export_data_restricts_profiles_of_restricted_columns() {
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
                    display: restricted
                    examples: [1]
                  - name: name
                    type: string
                    display: restricted
                    examples: [otter]
        "},
    );
    let (problems, export) = export_data(&dict);
    assert_eq!(
        problems.status(),
        Status::Ok,
        "{}",
        problems.render(common::SNAPSHOT_STYLE).join("\n")
    );
    let json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&export.unwrap()).unwrap()).unwrap();
    let columns = &json["tables"][0]["columns"];

    let id = &columns[0]["profile"];
    assert_eq!(id["distinct"]["count"], 3);
    assert!(id["range"].is_null());
    assert!(id["sample_values"].is_null());
    assert!(id["histogram"].is_null());

    let name = &columns[1]["profile"];
    assert_eq!(name["distinct"]["count"], 2);
    assert!(name["sample_values"].is_null());
    assert!(name["common_values"].is_null());
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

/// With at least one source file present, `export_auto` behaves as
/// `export_data`: the readable table profiles, and the tables whose sources
/// are missing or undeclared still warn.
#[test]
fn export_auto_profiles_when_any_source_exists() {
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
              - name: keepers
                source:
                  parquet: missing.parquet
                columns:
                  - name: keeper_id
                    type: number(id)
                    examples: [1]
              - name: visits
                columns:
                  - name: visit_id
                    type: number(id)
                    examples: [1]
        "},
    );
    let (problems, export) = export_auto(&dict);
    assert_eq!(problems.status(), Status::Warning);
    let json = serde_json::to_value(export.unwrap()).unwrap();
    assert_eq!(json["tables"][0]["rows"], 3);
    assert_eq!(
        json["tables"][0]["columns"][0]["profile"]["distinct"]["count"],
        3
    );
    assert!(json["tables"][1]["rows"].is_null());
    assert!(json["tables"][2]["rows"].is_null());
    let diagnostic = diagnostic(&dict, &problems.render(common::SNAPSHOT_STYLE).join("\n"));
    diagnostic.assert_contains(&["M05", "M04", "has no `source`"]);
    #[cfg(unix)]
    common::assert_snapshot!(diagnostic);
}

/// With sources declared but none of the files present, `export_auto` behaves
/// as `export_spec`: no data is read and no M04/M05 warnings are raised.
#[test]
fn export_auto_stays_quiet_when_no_source_is_readable() {
    let dir = temp_dir();
    let dict = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: animals
                source:
                  parquet: missing.parquet
                columns:
                  - name: id
                    type: number(id)
                    examples: [1]
              - name: keepers
                columns:
                  - name: keeper_id
                    type: number(id)
                    examples: [1]
        "},
    );
    let (problems, export) = export_auto(&dict);
    assert_eq!(problems.status(), Status::Ok);
    let json = serde_json::to_value(export.unwrap()).unwrap();
    assert!(json["tables"][0]["rows"].is_null());
    assert!(json["tables"][0]["columns"][0]["profile"].is_null());
}

/// With no sources declared at all, `export_auto` matches `export_spec`
/// exactly.
#[test]
fn export_auto_without_sources_matches_export_spec() {
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
    let (problems, export) = export_auto(&dict);
    assert_eq!(problems.status(), Status::Ok);
    let auto_json = serde_json::to_value(export.unwrap()).unwrap();
    let (_, spec_export) = export_spec(&dict);
    let spec_json = serde_json::to_value(spec_export.unwrap()).unwrap();
    assert_eq!(auto_json, spec_json);
}

/// The export never validates the data against the dictionary: a declared
/// column the data doesn't have simply gets no `profile`, and the rest of the
/// table still profiles.
#[test]
fn export_data_skips_columns_absent_from_the_data() {
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
                  - name: wings
                    type: number(quantity)
                    units: pairs
                    range: [0, 2]
        "},
    );
    let (problems, export) = export_data(&dict);
    assert_eq!(problems.status(), Status::Ok);
    let json = serde_json::to_value(export.unwrap()).unwrap();
    assert_eq!(json["tables"][0]["rows"], 3);
    let columns = &json["tables"][0]["columns"];
    assert_eq!(columns[0]["profile"]["distinct"]["count"], 3);
    assert!(columns[1]["profile"].is_null(), "no data, no profile");
}

/// Definitions export with their kind, value type, and references resolved:
/// a filter, a metric, a derived value, one definition building on another,
/// and an assertion that references a definition.
#[test]
fn definitions_export_resolved() {
    let json = export_json(indoc! {r#"
        $version: "0.1.0"
        tables:
          - name: orders
            columns:
              - name: status_cd
                type: number(id)
                examples: [90]
              - name: order_total
                type: number(quantity)
                units: usd
                range: [0, .inf]
              - name: tile_size
                type: string
                examples: ["Enterprise-1"]
            constraints:
              - assert: is_enterprise
            definitions:
              - name: net_revenue
                description: Realized revenue excluding returned orders.
                expr: SUM(CASE WHEN status_cd = 90 THEN 0 ELSE order_total END)
              - name: is_enterprise
                label: Enterprise segment
                expr: tile_size IN ('Mid-Market-3', 'Enterprise-1')
              - name: enterprise_revenue
                expr: SUM(CASE WHEN is_enterprise THEN order_total ELSE 0 END)
              - name: list_price
                expr: order_total * 1.2
    "#});
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    let defs = &json["tables"][0]["definitions"];

    assert_eq!(defs[0]["name"], "net_revenue");
    assert_eq!(defs[0]["kind"], "metric");
    assert_eq!(defs[0]["type"], "number");
    assert_eq!(
        defs[0]["description"],
        "<p>Realized revenue excluding returned orders.</p>"
    );
    assert_eq!(
        defs[0]["columns"],
        serde_json::json!(["status_cd", "order_total"])
    );
    assert!(defs[0].get("definitions").is_none());

    assert_eq!(defs[1]["kind"], "filter");
    assert_eq!(defs[1]["type"], "boolean");
    assert_eq!(defs[1]["label"], "Enterprise segment");
    assert_eq!(defs[1]["columns"], serde_json::json!(["tile_size"]));

    // A definition building on another names it under `definitions`; the
    // columns that definition reads stay on its own entry.
    assert_eq!(defs[2]["kind"], "metric");
    assert_eq!(defs[2]["definitions"], serde_json::json!(["is_enterprise"]));
    assert_eq!(defs[2]["columns"], serde_json::json!(["order_total"]));

    assert_eq!(defs[3]["kind"], "derived");
    assert_eq!(defs[3]["type"], "number");

    // An assertion referencing a definition lists it separately from columns.
    let assertion = &json["tables"][0]["constraints"][0];
    assert_eq!(
        assertion["definitions"],
        serde_json::json!(["is_enterprise"])
    );
    assert_eq!(assertion["columns"], serde_json::json!([]));

    // Definitions carry translations. A reference to another definition
    // renders as a bare name, as if it were a column — the client
    // substitutes the referenced definition's own translation.
    let filter_sql = &defs[1]["translations"][0];
    assert_eq!(filter_sql["target"], "SQL(duckdb)");
    assert!(filter_sql["code"].as_str().unwrap().contains("tile_size"));

    let metric_sql = defs[2]["translations"][0]["code"].as_str().unwrap();
    assert!(
        metric_sql.contains("\"is_enterprise\""),
        "reference rendered as a name: {metric_sql}"
    );
    assert!(metric_sql.contains("order_total"));

    // So does an assertion that references a definition.
    let assertion_sql = assertion["translations"][0]["code"].as_str().unwrap();
    assert!(
        assertion_sql.contains("\"is_enterprise\""),
        "reference rendered as a name: {assertion_sql}"
    );
}

/// The full export of a dictionary with definitions, so the shape is reviewable.
#[test]
fn definitions_export_snapshot() {
    insta::assert_snapshot!(export_json(indoc! {r#"
        $version: "0.1.0"
        tables:
          - name: orders
            columns:
              - name: status_cd
                type: number(id)
                examples: [90]
              - name: order_total
                type: number(quantity)
                units: usd
                range: [0, .inf]
              - name: tile_size
                type: string
                examples: ["Enterprise-1"]
            constraints:
              - assert: is_enterprise
            definitions:
              - name: net_revenue
                description: Realized revenue excluding returned orders.
                expr: SUM(CASE WHEN status_cd = 90 THEN 0 ELSE order_total END)
              - name: is_enterprise
                label: Enterprise segment
                expr: tile_size IN ('Mid-Market-3', 'Enterprise-1')
              - name: enterprise_revenue
                expr: SUM(CASE WHEN is_enterprise THEN order_total ELSE 0 END)
              - name: list_price
                expr: order_total * 1.2
    "#}));
}

/// An expression written in another language exports what the author wrote,
/// what it was read as, and how faithfully.
#[test]
fn an_r_expression_exports_both_forms() {
    insta::assert_snapshot!(export_json(indoc! {r#"
        $version: "0.1.0"
        $learn_more: http://data-dict.tidyverse.org/
        description: Each row is a survey response.
        tables:
          - name: survey
            columns:
              - name: score
                type: number(quantity)
                units: points
                range: [0, 100]
            constraints:
              - assert: round(score, 2) == score
                language: r
            definitions:
              - name: high
                expr: score > 80
                language: r
    "#}));
}

/// An expression in the dictionary's own language needed no reading, so it
/// carries none of the four keys — `expression` speaks for itself.
#[test]
fn a_data_dict_expression_exports_neither_language_nor_canonical() {
    let json = export_json(indoc! {r#"
        $version: "0.1.0"
        $learn_more: http://data-dict.tidyverse.org/
        description: Each row is a survey response.
        tables:
          - name: survey
            columns:
              - name: score
                type: number(quantity)
                units: points
                range: [0, 100]
            constraints:
              - assert: ROUND(score, 2) = score
    "#});
    assert!(!json.contains("\"language\""), "{json}");
    assert!(!json.contains("\"canonical\""), "{json}");
    assert!(!json.contains("\"fidelity\""), "{json}");
}
