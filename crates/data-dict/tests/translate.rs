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
