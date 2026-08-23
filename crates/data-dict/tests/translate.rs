//! Integration tests for `data_dict::translate`; see `site/export.md` for the
//! shape the output shares with the export document.

mod common;
use common::{temp_dir, write_dict};

use data_dict::translate::{Options, translate};
use indoc::indoc;

fn translate_json(yaml: &str) -> String {
    let path = write_dict(&temp_dir(), yaml);
    let translations = translate(&path, &Options::default()).unwrap_or_else(|problems| {
        panic!(
            "translate should succeed, got:\n{}",
            problems.render(common::SNAPSHOT_STYLE).join("\n")
        )
    });
    serde_json::to_string_pretty(&translations).unwrap()
}

/// A constraint that references a definition lists it under `definitions`,
/// not `columns` — only loadable columns appear in `columns`, matching the
/// export document.
#[test]
fn a_definition_reference_is_not_a_column() {
    insta::assert_snapshot!(translate_json(indoc! {r#"
        description: Each row is an order.
        tables:
          - name: orders
            columns:
              - name: tile_size
                type: string
                examples: ["Enterprise-1"]
              - name: order_total
                type: number(quantity)
                units: usd
                range: [0, .inf]
            constraints:
              - assert: is_enterprise AND order_total > 0
            definitions:
              - name: is_enterprise
                expr: tile_size IN ('Enterprise-1')
    "#}));
}

/// An expression written in R is read into the language, and the record says
/// so: `language` names where it came from and `canonical` gives the reading.
#[test]
fn an_r_expression_reports_its_language_and_canonical_form() {
    let path = write_dict(
        &temp_dir(),
        indoc! {r#"
            description: Each row is a survey response.
            tables:
              - name: survey
                columns:
                  - name: postcode
                    type: string
                    examples: ["NZ-1010"]
        "#},
    );
    let options = Options {
        expr: Some("nchar(postcode) <= 10".to_string()),
        from: Some("r".to_string()),
        targets: vec!["data-dict".to_string()],
        ..Options::default()
    };
    let translations = translate(&path, &options).expect("translates");
    insta::assert_snapshot!(serde_json::to_string_pretty(&translations).unwrap());
}

/// The data-dict language is a target, but never a default one: asking for no
/// target must not echo the expression back.
#[test]
fn the_language_is_not_among_the_default_targets() {
    let path = write_dict(
        &temp_dir(),
        indoc! {r#"
            description: Each row is an order.
            tables:
              - name: orders
                columns:
                  - name: qty
                    type: number(quantity)
                    units: items
                    range: [0, 100]
                constraints:
                  - assert: qty > 0
        "#},
    );
    let translations = translate(&path, &Options::default()).expect("translates");
    let targets: Vec<&str> = translations[0]
        .translations
        .iter()
        .map(|t| t.target)
        .collect();
    assert!(!targets.contains(&"data-dict"), "{targets:?}");
    // And a record that needed no reading carries neither language nor
    // canonical form: the expression is already in the language.
    assert!(translations[0].language.is_none());
    assert!(translations[0].canonical.is_none());
    assert!(translations[0].notes.is_empty());
}

/// Reading from another language makes the data-dict spelling the interesting
/// one, so it joins the default set exactly then.
#[test]
fn reading_from_another_language_adds_the_language_to_the_defaults() {
    let path = write_dict(
        &temp_dir(),
        indoc! {r#"
            description: Each row is an order.
            tables:
              - name: orders
                columns:
                  - name: qty
                    type: number(quantity)
                    units: items
                    range: [0, 100]
        "#},
    );
    let options = Options {
        expr: Some("qty > 0".to_string()),
        from: Some("r".to_string()),
        ..Options::default()
    };
    let translations = translate(&path, &options).expect("translates");
    let targets: Vec<&str> = translations[0]
        .translations
        .iter()
        .map(|t| t.target)
        .collect();
    assert!(targets.contains(&"data-dict"), "{targets:?}");
}

#[test]
fn an_unknown_source_language_lists_the_readable_ones() {
    let path = write_dict(
        &temp_dir(),
        indoc! {r#"
            description: Each row is an order.
            tables:
              - name: orders
                columns:
                  - name: qty
                    type: number(quantity)
                    units: items
                    range: [0, 100]
        "#},
    );
    let options = Options {
        expr: Some("qty > 0".to_string()),
        from: Some("perl".to_string()),
        ..Options::default()
    };
    let problems = translate(&path, &options).expect_err("no such language");
    let rendered = problems.render(common::SNAPSHOT_STYLE).join("\n");
    assert!(
        rendered.contains("unknown expression language"),
        "{rendered}"
    );
    assert!(rendered.contains("data-dict, r"), "{rendered}");
}

/// R that is well-formed but says something the language can't says which
/// construct, and that the rule has to be rewritten.
#[test]
fn an_untranslatable_construct_names_itself() {
    let path = write_dict(
        &temp_dir(),
        indoc! {r#"
            description: Each row is an order.
            tables:
              - name: orders
                columns:
                  - name: qty
                    type: number(quantity)
                    units: items
                    range: [0, 100]
        "#},
    );
    let options = Options {
        expr: Some("sapply(qty, is.na)".to_string()),
        from: Some("r".to_string()),
        ..Options::default()
    };
    let problems = translate(&path, &options).expect_err("no equivalent");
    let rendered = problems.render(common::SNAPSHOT_STYLE).join("\n");
    assert!(rendered.contains("`sapply()`"), "{rendered}");
    assert!(rendered.contains("no equivalent"), "{rendered}");
}
