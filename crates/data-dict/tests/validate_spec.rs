//! Integration tests for the `validate` entry point.
//!
//! Prefer inline YAML (an `indoc!` body passed to one of the `dict` helpers,
//! which prepend the boilerplate `$version`/`$learn_more` header) so each
//! case's shape sits next to its assertions. Reserve fixture files under
//! `tests/fixtures/{valid,invalid,spec}/` for the few cases too long to read
//! inline — chiefly the multi-table relationship checks (S01–S06). Those
//! fixtures double as runnable CLI inputs:
//!
//!     cargo run -p data-dict-cli -- validate-spec \
//!         crates/data-dict/tests/fixtures/spec/s01-fk-no-relationship.yaml

use std::path::{Path, PathBuf};

mod common;

use common::{Diagnostic, assert_snapshot};
use data_dict::Severity;
use indoc::indoc;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn fixture(rel: &str) -> PathBuf {
    fixtures_root().join(rel)
}

// --- inline helpers ------------------------------------------------------

/// Write `body` to a temp file beneath the boilerplate `$version`/`$learn_more`
/// header (see [`common::write_dict`]) and return its path. The header's two
/// lines mean `body` starts at line 3, which the snapshots reflect.
fn dict(body: &str) -> PathBuf {
    common::write_dict(&common::temp_dir(), body)
}

/// Write `yaml` verbatim — no header — to a temp file. For the few cases that
/// exercise the top-level `$version`/`$learn_more` keys themselves.
fn raw(yaml: &str) -> PathBuf {
    common::write_yaml(&common::temp_dir(), yaml)
}

fn assert_valid_dict(body: &str) {
    assert_valid(dict(body));
}

/// Assert `body` validates with neither errors nor warnings — entirely clean.
/// Stronger than [`assert_valid_dict`], which only checks for errors.
fn assert_clean_dict(body: &str) {
    assert_clean(dict(body));
}

fn assert_clean(path: PathBuf) {
    let errors = diagnostics(&path, Severity::Error);
    assert!(
        errors.is_empty(),
        "expected a clean document, but it errored:\n{}",
        errors.join("\n"),
    );
    let warnings = diagnostics(&path, Severity::Warning);
    assert!(
        warnings.is_empty(),
        "expected a clean document, but it warned:\n{}",
        warnings.join("\n"),
    );
}

fn assert_invalid_dict(body: &str, expected: &[&str]) {
    assert_invalid(dict(body), expected);
}

/// Validate the document at `path`, expected to fail, capturing its source and
/// rendered errors (temp path rewritten to the bare `dict.yaml`) for
/// snapshotting.
fn failing(path: &Path) -> Diagnostic {
    let errors = diagnostics(path, Severity::Error);
    assert!(
        !errors.is_empty(),
        "expected document to fail validation, but it passed"
    );
    common::diagnostic(path, &errors.join("\n"))
}

fn failing_dict(body: &str) -> Diagnostic {
    failing(&dict(body))
}

fn failing_raw(yaml: &str) -> Diagnostic {
    failing(&raw(yaml))
}

/// Validate the document at `path`, expected to pass *with* warnings, capturing
/// its source and rendered warnings for snapshotting.
fn warning(path: &Path) -> Diagnostic {
    assert!(
        diagnostics(path, Severity::Error).is_empty(),
        "expected document to validate, but it failed"
    );
    let warnings = diagnostics(path, Severity::Warning);
    assert!(
        !warnings.is_empty(),
        "expected document to emit a warning, but it was clean"
    );
    common::diagnostic(path, &warnings.join("\n"))
}

fn warning_dict(body: &str) -> Diagnostic {
    warning(&dict(body))
}

fn warning_raw(yaml: &str) -> Diagnostic {
    warning(&raw(yaml))
}

/// Render the problems of the given `severity` for a document, in source order.
/// Pre-flight failures (I/O, unparseable YAML, structural schema errors) are
/// error-severity problems like any other, so they surface here when collecting
/// errors and are skipped when collecting warnings.
fn diagnostics(path: &Path, severity: Severity) -> Vec<String> {
    let problems = data_dict::validate_spec(path);
    problems
        .items
        .iter()
        .filter(|p| p.severity == severity)
        .map(|p| p.to_text(&problems.source, common::SNAPSHOT_STYLE))
        .collect()
}

// --- fixture helpers -----------------------------------------------------

fn assert_valid(path: PathBuf) {
    let errors = diagnostics(&path, Severity::Error);
    assert!(
        errors.is_empty(),
        "expected {} to validate, but:\n{}",
        path.display(),
        errors.join("\n"),
    );
}

fn assert_invalid(path: PathBuf, expected: &[&str]) {
    let errors = diagnostics(&path, Severity::Error);
    assert!(
        !errors.is_empty(),
        "expected {} to fail validation, but it passed",
        path.display()
    );
    let text = errors.join("\n");
    for s in expected {
        assert!(
            text.contains(s),
            "expected {:?} in diagnostic for {}, got:\n{text}",
            s,
            path.display(),
        );
    }
}

/// Validate a fixture that must fail, returning the rendered diagnostic with
/// machine-specific noise removed so it can be snapshotted. Used for the
/// long-form `spec/` fixtures — any document expected to error.
///
/// Diagnostics are rendered with [`common::SNAPSHOT_STYLE`] (no terminal
/// styling, anonymized line numbers); the one remaining unstable bit is the
/// absolute on-disk path of the fixture, which [`common::sanitize`] rewrites to
/// its `tests/fixtures/`-relative form.
fn failing_diagnostic(rel: &str) -> Diagnostic {
    let path = fixture(rel);
    let errors = diagnostics(&path, Severity::Error);
    if errors.is_empty() {
        panic!("expected {rel} to fail validation, but it passed");
    }
    Diagnostic {
        source: std::fs::read_to_string(&path).unwrap(),
        rendered: common::sanitize(&errors.join("\n"), &fixtures_root()),
    }
}

/// [`failing_diagnostic`]'s counterpart for a fixture that validates but warns.
fn warning_diagnostic(rel: &str) -> Diagnostic {
    let path = fixture(rel);
    assert_valid(path.clone());
    let warnings = diagnostics(&path, Severity::Warning);
    if warnings.is_empty() {
        panic!("expected {rel} to warn, but it was clean");
    }
    Diagnostic {
        source: std::fs::read_to_string(&path).unwrap(),
        rendered: common::sanitize(&warnings.join("\n"), &fixtures_root()),
    }
}

// --- valid documents -----------------------------------------------------

// The smallest recommended document: the required `$version` plus the
// recommended `$learn_more` (both from the header), and no tables.
#[test]
fn minimal() {
    assert_clean_dict("");
}

// A column with only a `name` and no `type` is acknowledged but not described,
// so it is exempt from the S07 data-representation requirement.
#[test]
fn typeless_column_needs_no_representation() {
    assert_valid_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: label
                type: string
                examples: [a, b, c]
              - name: scratch
    "});
}

// A single-table dictionary that describes the dataset with the top-level
// name/description/details (leaving the table undescribed) is exactly what S16
// recommends, so it must validate without an S16 warning.
#[test]
fn top_level_description_no_s16() {
    assert_clean_dict(indoc! {"
        name: foodbank
        label: FoodData Central
        description: A snapshot of the USDA FoodData Central database.
        details: Includes both branded and foundation foods.
        tables:
          - name: food
            columns:
              - name: id
                label: FoodData Central ID
                type: number(id)
                examples: [1, 2, 3]
    "});
}

// `origin` is a loose, unenforced reference (a URL or a dictionary-relative
// path) accepted at both the dataset and table levels.
#[test]
fn origin_dataset_and_table() {
    assert_clean_dict(indoc! {"
        name: foodbank
        origin: https://github.com/example/foodbank/blob/main/data-raw/all.R
        tables:
          - name: food
            origin: data-raw/food.R
            columns:
              - name: id
                type: number(id)
                examples: [1, 2, 3]
    "});
}

// `origin` is not a column-level key: the closed column object rejects it.
#[test]
fn origin_on_column_rejected() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: food
            columns:
              - name: id
                type: number(id)
                examples: [1, 2, 3]
                origin: data-raw/food.R
    "});
    diagnostic.assert_contains(&["Unknown property 'origin'"]);
}

#[test]
fn restricted_display_is_valid() {
    assert_clean_dict(indoc! {"
        tables:
          - name: people
            columns:
              - name: ssn
                type: string
                display: restricted
                examples: [000-00-0000]
    "});
}

// --- warnings ------------------------------------------------------------

// A document missing the recommended `$learn_more` key validates (it is not an
// error) but surfaces a S09 warning.
#[test]
fn warn_missing_learn_more() {
    let diagnostic = warning_raw("$version: 0.1.0\n");
    diagnostic.assert_contains(&["S09", "$learn_more"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A single-table dictionary that puts `description`/`details` on the table
// rather than at the top level validates, but surfaces one S16 warning per
// misplaced key.
#[test]
fn warn_single_table_description() {
    let diagnostic = warning_dict(indoc! {"
        tables:
          - name: food
            label: Foods
            description: Each row is a food item.
            details: Collected from the USDA FoodData Central database.
            columns:
              - name: id
                type: number(id)
                examples: [1, 2, 3]
    "});
    diagnostic.assert_contains(&["S16", "label", "description", "details"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// --- structural (pre-flight) checks --------------------------------------
//
// Each invalid case asserts at two levels in one test: `assert_contains` checks
// the key phrases on every platform, and `assert_snapshot!` guards the exact
// rendered diagnostic on Unix only. The snapshot is Unix-gated because the
// upstream renderer measures Unicode box-drawing characters differently on
// Windows, shifting pointer arrows by one column; the cross-platform phrase
// check still runs there. Regenerate snapshots after intentional message
// changes with:
//
//     INSTA_UPDATE=always cargo test -p data-dict

#[test]
fn missing_version() {
    let diagnostic = failing_raw("tables: []\n");
    diagnostic.assert_contains(&["S18", "`$version` is not set"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn newer_spec_version() {
    let diagnostic = failing_raw("$version: 99.0.0\ntables: []\n");
    diagnostic.assert_contains(&["S32", "newer", "Upgrade data-dict"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn older_spec_version() {
    let diagnostic = failing_raw("$version: 0.0.9\ntables: []\n");
    diagnostic.assert_contains(&["S32", "starts at `0.1.0`", "not a valid spec version"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn malformed_spec_version() {
    let diagnostic = failing_raw("$version: latest\ntables: []\n");
    diagnostic.assert_contains(&["S32", "`latest` is not a valid version number"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn unknown_top_level_key() {
    let diagnostic = failing_dict("bogus: 1\n");
    diagnostic.assert_contains(&["Unknown property 'bogus'"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn bad_cardinality() {
    let diagnostic = failing_dict(indoc! {"
        relationships:
          - cardinality: many-to-many
            join: a.x = b.y
    "});
    diagnostic.assert_contains(&["many-to-many"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn non_string_glossary_value() {
    let diagnostic = failing_dict(indoc! {"
        glossary:
          term: 42
    "});
    diagnostic.assert_contains(&["Expected string"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn enum_non_string_label() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: status
                type: enum
                values: {active: 1, inactive: 2}
    "});
    diagnostic.assert_contains(&["Q-1-11", "Expected array, got object"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn unknown_display_value() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: people
            columns:
              - name: ssn
                type: string
                display: hidden
                examples: [000-00-0000]
    "});
    diagnostic.assert_contains(&["hidden", "restricted"]);
}

// --- relationship checks (S01–S06) ---------------------------------------
//
// These span two tables, so they stay as fixture files rather than inline YAML.
// Each snapshots its full rendered diagnostic: snapshotting the whole output
// (rather than asserting a single code is present) guards the exact set of
// findings — e.g. that `s03-missing-column` reports the missing column without
// *also* checking cardinality against it and emitting a redundant S06.

#[test]
fn clean_two_tables() {
    assert_valid(fixture("spec/clean-two-tables.yaml"));
}

#[test]
fn s01_fk_no_relationship() {
    assert_snapshot!(failing_diagnostic("spec/s01-fk-no-relationship.yaml"));
}

#[test]
fn s02_missing_table() {
    assert_snapshot!(failing_diagnostic("spec/s02-missing-table.yaml"));
}

#[test]
fn s02_alias_unknown_table() {
    assert_snapshot!(failing_diagnostic("spec/s02-alias-unknown-table.yaml"));
}

#[test]
fn s03_missing_column() {
    assert_snapshot!(failing_diagnostic("spec/s03-missing-column.yaml"));
}

#[test]
fn s04_bad_join() {
    assert_snapshot!(failing_diagnostic("spec/s04-bad-join.yaml"));
}

#[test]
fn s05_conflicts_not_on_both_sides() {
    assert_snapshot!(failing_diagnostic(
        "spec/s05-conflicts-not-on-both-sides.yaml"
    ));
}

// The opposite of the above: `amount` is genuinely a column on both tables (a
// real conflict) but is not declared in `conflicts`. S05 only checks declared
// entries, so this must validate cleanly rather than demanding the conflict be named.
#[test]
fn s05_undeclared_conflict_ok() {
    assert_valid(fixture("spec/s05-undeclared-conflict-ok.yaml"));
}

#[test]
fn s06_cardinality_mismatch() {
    assert_snapshot!(failing_diagnostic("spec/s06-cardinality-mismatch.yaml"));
}

// Recreated from the bundled `otters` example: a one-to-many self-join whose
// "one" side is not unique. Exercises the self-join orientation of S06.
#[test]
fn s06_self_join_one_to_many() {
    assert_snapshot!(failing_diagnostic("spec/s06-self-join-one-to-many.yaml"));
}

// Names that aren't plain identifiers are referenced from a `join` in
// backticks, so S02/S03 resolve them like any other name.
#[test]
fn quoted_names_ok() {
    assert_valid(fixture("spec/quoted-names-ok.yaml"));
}

// --- aliases (S25/S26/S27) -----------------------------------------------

#[test]
fn s25_unaliased_self_join() {
    assert_snapshot!(failing_diagnostic("spec/s25-unaliased-self-join.yaml"));
}

// One alias and one bare table name still leaves both sides standing for the
// same rows, so aliasing half the join isn't enough.
#[test]
fn s25_half_aliased_self_join() {
    assert_snapshot!(failing_diagnostic("spec/s25-half-aliased-self-join.yaml"));
}

// Two aliases of one table are two sides, so this is the self-join spelling the
// spec asks for. Also covers S01 resolving a foreign key through an alias.
#[test]
fn aliases_self_join_ok() {
    assert_clean(fixture("spec/aliases-self-join-ok.yaml"));
}

// Aliases are allowed where they aren't required: two tables joined twice, with
// the alias naming each role.
#[test]
fn aliases_role_playing_ok() {
    assert_clean(fixture("spec/aliases-role-playing-ok.yaml"));
}

#[test]
fn s26_alias_shadows_table() {
    assert_snapshot!(failing_diagnostic("spec/s26-alias-shadows-table.yaml"));
}

#[test]
fn s27_unused_alias() {
    assert_snapshot!(warning_diagnostic("spec/s27-unused-alias.yaml"));
}

// An alias resolves only within the relationship that declares it, so a second
// relationship naming it gets S02 rather than the first one's table.
#[test]
fn alias_does_not_leak_between_relationships() {
    assert_invalid_dict(
        indoc! {"
            tables:
              - name: a
                columns:
                  - name: id
                    type: number(id)
                    constraints: [primary_key]
                    examples: [1, 2]
              - name: b
                columns:
                  - name: a_id
                    type: number(id)
                    examples: [1, 2]

            relationships:
              - join: b.a_id = parent.id
                aliases: {parent: a}
                cardinality: many-to-one
              - join: b.a_id = parent.id
                cardinality: many-to-one
        "},
        &["S02"],
    );
}

// --- data representation (S07) -------------------------------------------

#[test]
fn s07_enum_without_values() {
    assert_snapshot!(failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: c
                type: enum
    "}));
}

#[test]
fn s07_range_type_missing_range() {
    assert_snapshot!(failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: weight
                type: number(quantity)
              - name: recorded_at
                type: date
    "}));
}

#[test]
fn s07_other_type_missing_examples() {
    assert_snapshot!(failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: label
                type: string
              - name: code
                type: number(id)
    "}));
}

// A `boolean` column carries no data representation key, so it must validate cleanly
// without `examples` — the one non-enum/range type exempt from S07's
// missing-`examples` check.
#[test]
fn s07_boolean_no_examples_ok() {
    assert_valid_dict(indoc! {"
        tables:
          - name: account
            columns:
              - name: id
                type: number(id)
                constraints: [primary_key]
                examples: [1, 2, 3, 4, 5]
              - name: is_active
                type: boolean
    "});
}

#[test]
fn s07_wrong_rep_on_enum() {
    assert_snapshot!(failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: status
                type: enum
                range: [0, 10]
    "}));
}

// `range` is only allowed on ordered numeric / date / datetime columns, not on
// strings. `examples` is supplied so the only finding is the misplaced `range`.
#[test]
fn s07_range_on_string_type() {
    assert_snapshot!(failing_dict(indoc! {r#"
        tables:
          - name: table
            columns:
              - name: c
                type: string
                examples: [a, z]
                range: ["a", "z"]
    "#}));
}

#[test]
fn s07_examples_on_boolean() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: active
                type: boolean
                examples: [true, false]
    "});
    diagnostic.assert_contains(&["S07", "type `boolean`", "examples"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// --- units (S08) ---------------------------------------------------------

// `units` is valid only on `number(quantity)`. A quantity column with units
// validates cleanly; units on any other type is S08.
#[test]
fn s08_units_ok_on_quantity() {
    assert_valid_dict(indoc! {"
        tables:
          - name: measurements
            columns:
              - name: mass
                type: number(quantity)
                units: g
                range: [0, 5000]
    "});
}

#[test]
fn s08_units_on_non_quantity() {
    assert_snapshot!(failing_dict(indoc! {"
        tables:
          - name: races
            columns:
              - name: finish_rank
                type: number(ordinal)
                units: place
                range: [1, 100]
    "}));
}

// --- names (S10, S11) ----------------------------------------------------

#[test]
fn s10_duplicate_column_name() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: id
                type: number(id)
                examples: [1, 2, 3]
              - name: id
                type: string
                examples: [a, b, c]
    "});
    diagnostic.assert_contains(&["S10", "Column names must be unique", "is duplicated"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// Table names must be unique across the dictionary. This was structurally
// guaranteed while tables were a map keyed by name; as a list of `name`d
// descriptors it is S10's job, mirroring the column case.
#[test]
fn s10_duplicate_table_name() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: food
            columns:
              - name: id
                type: number(id)
                examples: [1, 2, 3]
          - name: food
            columns:
              - name: id
                type: number(id)
                examples: [1, 2, 3]
    "});
    diagnostic.assert_contains(&["S10", "Table names must be unique", "is duplicated"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn s11_empty_table_name() {
    let diagnostic = failing_dict(indoc! {r#"
        tables:
          - name: ""
            columns:
              - name: id
                type: number(id)
                examples: [1, 2, 3]
    "#});
    diagnostic.assert_contains(&["S11", "table name is empty"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn s11_empty_column_name() {
    let diagnostic = failing_dict(indoc! {r#"
        tables:
          - name: table
            columns:
              - name: ""
                type: string
                examples: [a, b, c]
    "#});
    diagnostic.assert_contains(&["S11", "the `name` is empty"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// --- representation values (S12, S13) ------------------------------------

#[test]
fn s12_wrong_value_type() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: count
                type: number
                examples: [1, two, 3]
    "});
    diagnostic.assert_contains(&["S12", "must be a number"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S12: a `string` column's examples are strings, so a zip code written bare is
// a number, with the same quoting hint S24 gives a category.
#[test]
fn s12_unquoted_string_example() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: zip
                type: string
                examples: ['02134', 94110]
    "});
    diagnostic.assert_contains(&["S12", "must be a string", "`'94110'`"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// The same finding with the examples written one per line, where the offending
// value sits several lines below the key that introduces it.
#[test]
fn s12_unquoted_string_example_block_form() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: zip
                type: string
                examples:
                  - '02134'
                  - '94110'
                  - 60614
                  - '98101'
    "});
    diagnostic.assert_contains(&["S12", "must be a string", "`'60614'`"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn s12_date_not_iso() {
    let diagnostic = failing_dict(indoc! {r#"
        tables:
          - name: table
            columns:
              - name: seen_on
                type: date
                range: ["2020-01-01", "20-01-2021"]
    "#});
    diagnostic.assert_contains(&["S12", "ISO 8601 date"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn s12_datetime_requires_timezone_errors() {
    assert_invalid_dict(
        indoc! {r#"
            tables:
              - name: table
                columns:
                  - name: seen_at
                    type: datetime
                    range: ["2024-01-31T09:30:00", "2024-02-01T09:30:00"]
        "#},
        &["S12", "timezone"],
    );
}

#[test]
fn s13_descending_range() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: mass
                type: number(quantity)
                units: kg
                range: [100, 10]
    "});
    diagnostic.assert_contains(&["S13", "is greater than the maximum"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// Guards that valid representation values and ascending ranges across every
// type — including quoted numeric-looking strings and a boolean with no
// representation key — produce no S07/S12/S13 noise. Stays a fixture for length.
#[test]
fn s12_s13_valid_ok() {
    assert_valid(fixture("spec/s12-s13-valid-ok.yaml"));
}

// An open-ended range: `-.inf`/`.inf` leave a bound open on any range type,
// including temporal columns whose other bound is an ISO 8601 string.
#[test]
fn s12_s13_infinite_bounds_ok() {
    assert_valid_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: mass
                type: number(quantity)
                units: kg
                range: [0, .inf]
              - name: seen_on
                type: date
                range: [2019-04-01, .inf]
              - name: seen_at
                type: datetime
                range: [-.inf, \"2024-02-01T00:00:00Z\"]
    "});
}

// `.nan` reads as a number but has no place on the number line, so it can
// neither bound a range nor stand for an observed value.
#[test]
fn s12_nan_is_not_a_bound() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: mass
                type: number(quantity)
                units: kg
                range: [.nan, 10]
    "});
    diagnostic.assert_contains(&[
        "S12",
        "must have a place on the number line",
        "is `.nan`",
        "leave that end of the range open",
    ]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn s12_nan_is_not_a_maximum_either() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: mass
                type: number(quantity)
                units: kg
                range: [0, .nan]
    "});
    diagnostic.assert_contains(&["S12", "must have a place on the number line"]);
    // A NaN bound is rejected before S13 compares the two ends, so a range it
    // makes uncomparable is reported once, not twice.
    assert!(!diagnostic.rendered.contains("S13"));
}

#[test]
fn s12_nan_is_not_an_example() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: ratio
                type: number
                examples: [1.5, .nan, 3.5]
    "});
    diagnostic.assert_contains(&["S12", "must have a place on the number line"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// `.inf` as a minimum runs backwards even on a temporal column, where the
// maximum is a finite ISO 8601 date.
#[test]
fn s13_infinite_bound_wrong_end() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: seen_on
                type: date
                range: [.inf, 2019-04-01]
    "});
    diagnostic.assert_contains(&["S13", "is greater than the maximum"]);
}

// --- enum values (S24) ---------------------------------------------------

// S24: a value no quoting rescues, so it carries no hint.
#[test]
fn s24_null_enum_value() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: grade
                type: enum
                values: {pass: Pass, ~: Unknown}
    "});
    diagnostic.assert_contains(&["S24", "is null"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A category coded as a number is a string once quoted.
#[test]
fn s24_numeric_codes_ok() {
    assert_valid_dict(indoc! {r#"
        tables:
          - name: table
            columns:
              - name: grade
                type: enum
                values: ["1", "2", "3"]
    "#});
}

// S24 applies to `list(enum)` exactly as to a scalar enum.
#[test]
fn s24_empty_values_on_list_enum() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: categories
                type: list(enum)
                values: []
    "});
    diagnostic.assert_contains(&["S24", "is empty"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S24: written unquoted, the same codes are numbers, and the hint says so.
#[test]
fn s24_unquoted_numeric_codes() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: correction
                type: enum
                values:
                  1: No correction
                  0.974: Curvilinear correction
    "});
    diagnostic.assert_contains(&["S24", "is a number", "`'0.974'`"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn s24_unquoted_boolean_value() {
    assert_invalid_dict(
        indoc! {"
            tables:
              - name: table
                columns:
                  - name: consent
                    type: enum
                    values: [true, false, unknown]
        "},
        &["S24", "is a boolean", "`'true'`"],
    );
}

// Numeric-looking keys must not make the map form read as a list.
#[test]
fn s24_mixed_key_types_map_form_ok() {
    assert_valid_dict(indoc! {r#"
        tables:
          - name: table
            columns:
              - name: thermocline
                type: enum
                values:
                  '-9': Not known
                  N: 'No'
                  Y: 'Yes'
    "#});
}

// S24: an empty `values` permits nothing, in either form.
#[test]
fn s24_empty_enum_values() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: table
            columns:
              - name: grade
                type: enum
                values: []
    "});
    diagnostic.assert_contains(&["S24", "is empty"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn s24_empty_enum_values_map_form() {
    assert_invalid_dict(
        indoc! {"
            tables:
              - name: table
                columns:
                  - name: grade
                    type: enum
                    values: {}
        "},
        &["S24", "is empty"],
    );
}

// --- todo (S31) ------------------------------------------------------------

// Every remaining `todo` warns, wherever it sits: the dataset, a table, a
// column, a struct field, and a relationship — one warning per note.
#[test]
fn s31_todo_at_every_level() {
    let diagnostic = warning_dict(indoc! {"
        todo: Describe the dataset as a whole.
        tables:
          - name: food
            todo: Confirm the grain with the data team.
            columns:
              - name: id
                type: number(id)
                constraints: [primary_key]
                examples: [1, 2, 3]
              - name: status
                type: string
                examples: [active, closed]
                todo: Looks like an enum — confirm the full set of values.
              - name: addr
                type: struct
                fields:
                  - name: zip
                    type: string
                    examples: ['97201']
                    todo: Is this always 5 digits?
            definitions:
              - name: is_active
                expr: status = 'active'
                todo: Confirm the active status codes.
          - name: category
            columns:
              - name: id
                type: number(id)
                constraints: [primary_key]
                examples: [1, 2]
              - name: food_id
                type: number(id)
                constraints: [foreign_key]
                examples: [1, 2]
        relationships:
          - join: category.food_id = food.id
            cardinality: many-to-one
            todo: Check whether this is really many-to-one.
    "});
    diagnostic.assert_contains(&["S31", "unresolved todo"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A multi-task todo is one string: a literal block scalar keeps a bulleted
// list readable, and it still warns once per key.
#[test]
fn s31_block_todo_with_bullets() {
    let diagnostic = warning_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: string
                examples: [x, y]
                todo: |
                  - Add a `description`.
                  - Specify `constraints`. Some options suggested by the data:
                    - constraints: [required] # all values present
    "});
    diagnostic.assert_contains(&["S31", "unresolved todo"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A `todo` is a single string; the list form is rejected structurally.
#[test]
fn todo_list_rejected() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: string
                examples: [x, y]
                todo:
                  - Write the description.
                  - Check whether nulls are allowed.
    "});
    diagnostic.assert_contains(&["Expected string"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A `todo`'s notes are strings; anything else is rejected structurally.
#[test]
fn todo_non_string_rejected() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: string
                examples: [x]
                todo: 42
    "});
    diagnostic.assert_contains(&["Expected"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// --- version (S17) -------------------------------------------------------

// The three valid forms of the optional top-level `version`: a date, a
// (quoted) version number, and an opaque hash.
#[test]
fn version_date_ok() {
    assert_valid_dict(indoc! {"
        version:
          date: 2024-01-31
    "});
}

#[test]
fn version_number_ok() {
    // Quoted so its exact text (1.10, not 1.1) survives YAML parsing.
    assert_valid_dict(indoc! {r#"
        version:
          number: "1.10.0"
    "#});
}

// A `number` may carry a semver pre-release and/or build suffix.
#[test]
fn version_number_suffix_ok() {
    assert_valid_dict(indoc! {r#"
        version:
          number: "1.2.0-rc.1+build.5"
    "#});
}

#[test]
fn version_hash_ok() {
    assert_valid_dict(indoc! {"
        version:
          hash: a1b2c3d
    "});
}

#[test]
fn s17_multiple_keys() {
    let diagnostic = failing_dict(indoc! {"
        version:
          date: 2024-01-31
          hash: a1b2c3d
    "});
    diagnostic.assert_contains(&["S17", "exactly one", "`date` has already been supplied"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn s17_empty_errors() {
    assert_invalid_dict(
        indoc! {"
            version: {}
        "},
        &["S17", "exactly one", "names none"],
    );
}

#[test]
fn s17_date_not_iso() {
    let diagnostic = failing_dict(indoc! {r#"
        version:
          date: "31/01/2024"
    "#});
    diagnostic.assert_contains(&["S17", "ISO 8601 date", "31/01/2024"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A `number` with too many components stays a string, so the diagnostic echoes
// the offending text.
#[test]
fn s17_number_not_three_components() {
    let diagnostic = failing_dict(indoc! {r#"
        version:
          number: "1.2.0.0"
    "#});
    diagnostic.assert_contains(&[
        "S17",
        "three dot-separated numeric components",
        "`1.2.0.0` is not a valid version number",
    ]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A two-component `number` is coerced to a YAML float, so it can't be echoed;
// the rule still flags it.
#[test]
fn s17_number_too_few_components_errors() {
    assert_invalid_dict(
        indoc! {"
            version:
              number: 1.2
        "},
        &["S17", "three dot-separated numeric components"],
    );
}

// The schema fixes `version`'s shape, so an unknown kind or a non-map value
// fails structurally (pre-flight) rather than at S17.
#[test]
fn version_unknown_key_errors() {
    assert_invalid_dict(
        indoc! {"
            version:
              tag: release-7
        "},
        &["Unknown property 'tag'"],
    );
}

#[test]
fn version_not_a_map_errors() {
    assert_invalid_dict(
        indoc! {"
            version: 2024-01-31
        "},
        &["object"],
    );
}

// --- time zones (S14, S15) -----------------------------------------------

// `time_zone` is valid only on `datetime`. A datetime column with a time zone —
// whose range is then written zoneless — validates cleanly; a time zone on any
// other type is S14.
#[test]
fn s14_time_zone_ok_on_datetime() {
    assert_valid_dict(indoc! {"
        tables:
          - name: events
            columns:
              - name: observed_at
                type: datetime
                time_zone: UTC
                range: [2020-01-01T00:00:00, 2024-12-31T23:59:59]
    "});
}

#[test]
fn s14_time_zone_on_non_datetime() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: events
            columns:
              - name: event_day
                type: date
                time_zone: America/New_York
                range: [2020-01-01, 2024-12-31]
    "});
    diagnostic.assert_contains(&["S14", "type `date`"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A `time_zone` outside the accepted shape (bare abbreviation, unknown area) is
// rejected by S15, which names the offending value.
#[test]
fn s15_bad_time_zone() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: events
            columns:
              - name: observed_at
                type: datetime
                time_zone: PST
                range: [2020-01-01T00:00:00, 2024-12-31T23:59:59]
    "});
    diagnostic.assert_contains(&["S15", "is not a valid time zone"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// --- list and struct types (S07, S28, S29) --------------------------------

#[test]
fn struct_with_fields_ok() {
    assert_clean_dict(indoc! {"
        tables:
          - name: deliveries
            columns:
              - name: id
                type: number(id)
                constraints: [primary_key]
                examples: [1, 2, 3]
              - name: address
                type: struct
                fields:
                  - name: street
                    type: string
                    examples: [123 Main St, 456 Oak Ave]
                  - name: zip
                    type: string
                    examples: ['97201', '78701']
    "});
}

#[test]
fn list_string_with_examples_ok() {
    assert_clean_dict(indoc! {"
        tables:
          - name: posts
            columns:
              - name: id
                type: number(id)
                constraints: [primary_key]
                examples: [1, 2, 3]
              - name: tags
                type: list(string)
                examples: [nature, outdoor, urban]
    "});
}

#[test]
fn list_enum_with_values_ok() {
    assert_clean_dict(indoc! {"
        tables:
          - name: products
            columns:
              - name: id
                type: number(id)
                constraints: [primary_key]
                examples: [1, 2, 3]
              - name: categories
                type: list(enum)
                values: [food, drink, dessert]
    "});
}

#[test]
fn list_quantity_with_range_ok() {
    assert_clean_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: id
                type: number(id)
                constraints: [primary_key]
                examples: [1, 2, 3]
              - name: prices
                type: list(number(quantity))
                units: USD
                range: [0.99, 999.99]
    "});
}

// Lists nest to any depth; the properties follow the innermost element type —
// `units` and `range` for quantities, `time_zone` for datetimes.
#[test]
fn nested_list_properties_follow_innermost_type() {
    assert_clean_dict(indoc! {"
        tables:
          - name: sensors
            columns:
              - name: temperature_grid
                type: list(list(number(quantity)))
                units: °C
                range: [-40, 60]
              - name: reading_batches
                type: list(list(datetime))
                time_zone: UTC
                range: [2020-01-01T00:00:00, 2024-12-31T23:59:59]
              - name: label_matrix
                type: list(list(enum))
                values: [hot, cold]
              - name: cell_groups
                type: list(list(struct))
                fields:
                  - name: value
                    type: number
                    examples: [1.5, 2.5]
    "});
}

// S28: the innermost element type is what must be recognised.
#[test]
fn s28_invalid_nested_list_element_type() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: grid
                type: list(list(foo))
                examples: [a, b]
    "});
    diagnostic.assert_contains(&["S28", "`foo` is not a recognised list element type"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A numeric or temporal column may give both `range` and `examples` — the
// extremes and a typical value say different things. Each type still requires
// its own key; the other is optional, whichever way round.
#[test]
fn s07_numeric_and_temporal_may_pair_range_and_examples() {
    assert_clean_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: plain
                type: number
                range: [1, 99]
                examples: [1, 20, 50, 70, 99]
              - name: ident
                type: number(id)
                range: [1, 500]
                examples: [1, 250, 500]
              - name: weight
                type: number(quantity)
                units: kg
                range: [0.5, 9.5]
                examples: [0.5, 3.2, 9.5]
              - name: seen_on
                type: date
                range: [2020-01-01, 2024-12-31]
                examples: [2020-01-01, 2022-06-15, 2024-12-31]
              - name: readings
                type: list(number)
                range: [0, 10]
                examples: [1, 5, 9]
    "});
}

// `string` is the one type left out of the pair: its bounds would order
// lexicographically, which is rarely what a reader wants.
#[test]
fn s07_string_may_not_pair_a_range() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: word
                type: string
                range: [a, z]
                examples: [a, m, z]
    "});
    diagnostic.assert_contains(&["S07", "`string` column must not use `range`"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// An optional `range` is checked as thoroughly as a required one: its bounds
// must match the column's type (S12) and run in order (S13).
#[test]
fn s12_optional_range_on_a_number_is_type_checked() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: n
                type: number
                range: [low, 9]
                examples: [1, 5, 9]
    "});
    diagnostic.assert_contains(&[
        "S12",
        "Both `range` values of a `number` column must be a number",
    ]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn s13_optional_range_on_a_number_must_be_ordered() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: n
                type: number
                range: [99, 1]
                examples: [1, 50, 99]
    "});
    diagnostic.assert_contains(&["S13", "minimum must be less than or equal to its maximum"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S07: a nested list resolves to its innermost element type, so a list of
// quantities still requires a `range`. The `examples` beside it are legal for a
// numeric type, so the absent `range` is the only finding.
#[test]
fn s07_nested_list_wrong_representation() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: grid
                type: list(list(number(quantity)))
                units: kg
                examples: [1, 2]
    "});
    diagnostic.assert_contains(&["S07", "list(list(number(quantity)))", "range"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S12: values are typed against the innermost element type.
#[test]
fn s12_nested_list_wrong_value_type() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: grid
                type: list(list(number(quantity)))
                units: kg
                range: [0, top]
    "});
    diagnostic.assert_contains(&["S12", "list(list(number(quantity)))"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn list_struct_with_fields_ok() {
    assert_clean_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: id
                type: number(id)
                constraints: [primary_key]
                examples: [1, 2, 3]
              - name: line_items
                type: list(struct)
                fields:
                  - name: product_id
                    type: number(id)
                    examples: [101, 204, 389]
                  - name: quantity
                    type: number(quantity)
                    units: units
                    range: [1, 100]
    "});
}

#[test]
fn list_boolean_no_representation_ok() {
    assert_clean_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: id
                type: number(id)
                constraints: [primary_key]
                examples: [1, 2, 3]
              - name: flags
                type: list(boolean)
    "});
}

// S28: list(foo) names the bad element type, not the whole list type.
#[test]
fn s28_invalid_list_element_type() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: c
                type: list(foo)
                examples: [a, b, c]
    "});
    diagnostic.assert_contains(&["S28", "foo", "element type"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S28: an unrecognised type string is rejected.
#[test]
fn s28_invalid_type() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: c
                type: foobar
                examples: [1, 2, 3]
    "});
    diagnostic.assert_contains(&["S28", "foobar"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S07: a struct column without fields is an error.
#[test]
fn s07_struct_without_fields() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: addr
                type: struct
    "});
    diagnostic.assert_contains(&["S07", "struct", "fields"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S07: `fields` on a non-struct column is an error.
#[test]
fn s07_fields_on_non_struct() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: tags
                type: list(string)
                examples: [a, b, c]
                fields:
                  - name: x
                    type: string
                    examples: [a]
    "});
    diagnostic.assert_contains(&["S07", "list(string)", "fields"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S07: a list(string) column without examples is an error.
#[test]
fn s07_list_missing_representation() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: tags
                type: list(string)
    "});
    diagnostic.assert_contains(&["S07", "list(string)", "examples"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S29: primary_key on a struct column is an error.
#[test]
fn s29_primary_key_on_struct() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: addr
                type: struct
                constraints: [primary_key]
                fields:
                  - name: street
                    type: string
                    examples: [123 Main St]
    "});
    diagnostic.assert_contains(&["S29", "primary_key", "struct"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S29: foreign_key on a list column is an error.
#[test]
fn s29_foreign_key_on_list() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: tags
                type: list(string)
                constraints: [foreign_key]
                examples: [a, b, c]
    "});
    diagnostic.assert_contains(&["S29", "foreign_key", "list(string)"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S29: unique on a list column is an error.
#[test]
fn s29_unique_on_list() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: tags
                type: list(string)
                constraints: [unique]
                examples: [a, b, c]
    "});
    diagnostic.assert_contains(&["S29", "unique", "list(string)"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// Fields are reduced column descriptors: `constraints` (like `label` and
// `display`) is rejected structurally by the schema.
#[test]
fn constraints_banned_on_struct_field() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: addr
                type: struct
                fields:
                  - name: id
                    type: number(id)
                    constraints: [primary_key]
                    examples: [1, 2, 3]
    "});
    diagnostic.assert_contains(&["Unknown property 'constraints'"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// Deep nesting stays structurally checked: the field schema recurses, so a
// banned property is caught on a field of a field.
#[test]
fn display_banned_on_nested_struct_field() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: addr
                type: struct
                fields:
                  - name: geo
                    type: struct
                    fields:
                      - name: lat
                        type: number
                        display: restricted
                        examples: [45.5]
    "});
    diagnostic.assert_contains(&["Unknown property 'display'"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// Struct fields are themselves validated (e.g. S12 catches wrong value types).
#[test]
fn struct_field_s12_wrong_type() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: addr
                type: struct
                fields:
                  - name: zip
                    type: number(id)
                    examples: [not-a-number]
    "});
    diagnostic.assert_contains(&["S12"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// --- constraints (column & table assertions) -----------------------------
//
// The schema fixes only the *shape* of constraints: a column entry is either a
// structural bareword or an assertion map (`assert` + optional `description`),
// and a table entry is an assertion map only. The `assert` expression itself is
// then validated semantically — parsed (S19), its columns resolved (S20), and
// type-checked (S21) — by `assert_expr` (see that module for the grammar).

// A column may mix structural barewords with assertion maps in one list, and an
// assertion may carry an optional `description`.
#[test]
fn constraints_column_mixed_structural_and_assertion() {
    assert_valid_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: postcode
                type: string
                examples: [AB1 2CD]
                constraints:
                  - required
                  - assert: LENGTH(postcode) <= 10
                    description: Postcodes are at most ten characters.
    "});
}

// Table-level constraints are a list of assertion maps, the natural home for
// rules that span columns.
#[test]
fn constraints_table_assertions() {
    assert_valid_dict(indoc! {"
        tables:
          - name: survey
            columns:
              - name: start_date
                type: date
                range: [2000-01-01, 2030-01-01]
              - name: end_date
                type: date
                range: [2000-01-01, 2030-01-01]
            constraints:
              - assert: end_date >= start_date
                description: A contract can't end before it starts.
              - assert: COLUMNS(*) IS NOT NULL
    "});
}

// S19: an `assert` expression that fails to parse points at the failing token.
#[test]
fn constraints_s19_syntax_error() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: string
                examples: [x]
                constraints:
                  - assert: LENGTH(a) <=
    "});
    diagnostic.assert_contains(&["S19", "does not parse"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S19: an integer literal too large for the language's 64-bit integers. The
// language has no wider representation, so this is a syntax error rather than a
// silent widening to a float.
#[test]
fn constraints_s19_integer_literal_too_large() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: number
                examples: [1, 2]
                constraints:
                  - assert: a < 9223372036854775808
    "});
    diagnostic.assert_contains(&["S19", "too large for a 64-bit integer"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// Shifting a `date` by an interval gives a datetime, so a sub-day unit is
// meaningful rather than silently truncated, and the result still compares
// against the date it came from.
#[test]
fn constraints_date_shifted_by_a_sub_day_interval() {
    assert_valid_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: d
                type: date
                range: [2000-01-01, 2030-01-01]
                constraints:
                  - assert: d + interval(12, hours) >= d
    "});
}

// S20: an assertion referencing a column not on the table.
#[test]
fn constraints_s20_unknown_column() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: number
                examples: [1, 2]
            constraints:
              - assert: a > b
    "});
    diagnostic.assert_contains(&["S20", "`b`", "not found"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// An assertion may reach a struct's fields with dot access, at any depth.
#[test]
fn constraints_field_access_ok() {
    assert_valid_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: addr
                type: struct
                constraints:
                  - assert: LENGTH(addr.zip) = 5
                fields:
                  - name: zip
                    type: string
                    examples: ['97201']
                  - name: geo
                    type: struct
                    fields:
                      - name: lat
                        type: number
                        examples: [45.5]
            constraints:
              - assert: addr.geo.lat BETWEEN -90 AND 90
    "});
}

// S20: a field access naming a field the struct doesn't declare.
#[test]
fn constraints_s20_unknown_field() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: addr
                type: struct
                fields:
                  - name: zip
                    type: string
                    examples: ['97201']
            constraints:
              - assert: LENGTH(addr.zpi) = 5
    "});
    diagnostic.assert_contains(&["S20", "no field `zpi`"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S21: a list's elements can't be reached with dot access.
#[test]
fn constraints_s21_field_access_through_list() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: items
                type: list(struct)
                fields:
                  - name: qty
                    type: number(quantity)
                    units: units
                    range: [1, 10]
            constraints:
              - assert: items.qty > 0
    "});
    diagnostic.assert_contains(&["S21", "a list's elements can't be reached"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S20: a `COLUMNS([...])` list naming a column that does not exist.
#[test]
fn constraints_s20_unknown_column_in_columns_list() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: number
                examples: [1, 2]
            constraints:
              - assert: COLUMNS([a, missing]) IS NOT NULL
    "});
    diagnostic.assert_contains(&["S20", "`missing`"]);
}

// Backticks are optional on a name that doesn't need them: a quoted name is
// matched exactly like a bare one.
#[test]
fn constraints_quoting_does_not_change_matching() {
    assert_valid_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: qty
                type: number
                examples: [1, 2]
                constraints:
                  - assert: '`qty` > 0'
    "});
}

// S20: a quoted name resolves against the table like any other, so one that
// isn't there is still unknown.
#[test]
fn constraints_s20_unknown_quoted_column() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: number
                examples: [1, 2]
            constraints:
              - assert: '`no such column` IS NOT NULL'
    "});
    diagnostic.assert_contains(&["S20", "`no such column`", "not found"]);
}

// S19: a backtick left unclosed is a syntax error like any other.
#[test]
fn constraints_s19_unterminated_quoted_name() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: number
                examples: [1, 2]
            constraints:
              - assert: '`a IS NOT NULL'
    "});
    diagnostic.assert_contains(&["S19", "unterminated quoted name"]);
}

// S21: a type mismatch — a numeric length compared as if the column were a
// string, and a non-boolean assertion at the top level.
#[test]
fn constraints_s21_type_mismatch() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: qty
                type: number
                examples: [1, 2]
                constraints:
                  - assert: LENGTH(qty) <= 10
    "});
    diagnostic.assert_contains(&["S21", "LENGTH"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S21: an assertion whose whole expression is not boolean.
#[test]
fn constraints_s21_non_boolean() {
    assert_invalid_dict(
        indoc! {"
            tables:
              - name: t
                columns:
                  - name: qty
                    type: number
                    examples: [1, 2]
                    constraints:
                      - assert: qty
        "},
        &["S21", "boolean"],
    );
}

// S21: at most one `COLUMNS(...)` may appear in an assertion.
#[test]
fn constraints_s21_two_columns() {
    assert_invalid_dict(
        indoc! {"
            tables:
              - name: t
                columns:
                  - name: a
                    type: number
                    examples: [1, 2]
                  - name: b
                    type: number
                    examples: [1, 2]
                constraints:
                  - assert: COLUMNS(*) IS NOT NULL AND COLUMNS('a') > 0
        "},
        &["S21", "at most one"],
    );
}

// S21: a malformed `SIMILAR TO` regular expression.
#[test]
fn constraints_s21_bad_regex() {
    assert_invalid_dict(
        indoc! {"
            tables:
              - name: t
                columns:
                  - name: a
                    type: string
                    examples: [x]
                    constraints:
                      - assert: a SIMILAR TO '('
        "},
        &["S21", "regular expression"],
    );
}

// A date column may be compared against an ISO date string literal.
#[test]
fn constraints_date_literal_comparison_ok() {
    assert_valid_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: d
                type: date
                range: [2000-01-01, 2030-01-01]
                constraints:
                  - assert: d >= '2000-01-01'
    "});
}

// An `enum`'s values are its categories, so an enum is a string and takes the
// string functions, whatever its values look like.
#[test]
fn constraints_enum_is_a_string() {
    assert_valid_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: sex
                type: enum
                values: [M, F, U]
                constraints:
                  - assert: LENGTH(sex) = 1
              - name: grade
                type: enum
                values: ['1', '2', '3']
                constraints:
                  - assert: grade LIKE '_'
    "});
}

// S21: an enum is a string even when its values look like numbers, so numeric
// comparisons don't apply.
#[test]
fn constraints_s21_enum_compared_with_number() {
    assert_invalid_dict(
        indoc! {"
            tables:
              - name: t
                columns:
                  - name: grade
                    type: enum
                    values: [1, 2, 3]
                    constraints:
                      - assert: grade > 0
        "},
        &["S21", "a string", "a number"],
    );
}

// S23: a column listed by name only has no type, so it can't be used where one
// matters.
#[test]
fn constraints_s23_untyped_column() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: number
                examples: [1, 2]
              - name: notes
            constraints:
              - assert: notes > a
    "});
    diagnostic.assert_contains(&["S23", "`notes`", "no declared type"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// An untyped column is fine where no type is needed: a null test asks nothing of
// its operand, and neither does `COLUMNS(*) IS NOT NULL`.
#[test]
fn constraints_untyped_column_needs_no_type_for_null_tests() {
    assert_valid_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: number
                examples: [1, 2]
              - name: notes
            constraints:
              - assert: notes IS NOT NULL
              - assert: COLUMNS(*) IS NOT NULL
    "});
}

// S22: a `COLUMNS('<regex>')` that matches no column is a warning, not an error.
#[test]
fn constraints_s22_columns_regex_matches_nothing() {
    let diagnostic = warning_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: number
                examples: [1, 2]
            constraints:
              - assert: COLUMNS('zzz_nope') IS NOT NULL
    "});
    diagnostic.assert_contains(&["S22", "matches no columns"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// S21: a `COLUMNS(...)` selection is type-checked per matched column, so
// applying LENGTH to a matched numeric column is an error.
#[test]
fn constraints_s21_columns_wrong_type() {
    assert_invalid_dict(
        indoc! {"
            tables:
              - name: t
                columns:
                  - name: amount_paid
                    type: number
                    examples: [1, 2]
                constraints:
                  - assert: LENGTH(COLUMNS('amount')) > 0
        "},
        &["S21", "amount_paid"],
    );
}

// S30: an aggregate applied to something already aggregated.
#[test]
fn constraints_s30_nested_aggregate() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: qty
                type: number
                examples: [1, 2]
            constraints:
              - assert: AVG(MIN(qty)) > 0
    "});
    diagnostic.assert_contains(&["S30", "already an aggregate"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// Aggregates and row-level operands may be mixed freely; only nesting is wrong.
#[test]
fn constraints_aggregates_are_valid() {
    assert_clean_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: qty
                type: number
                examples: [1, 2]
              - name: region
                type: string
                examples: [north, south]
              - name: notes
              - name: flag
                type: boolean
            constraints:
              - assert: qty <= 2 * MIN(qty)
              - assert: COUNT_DISTINCT(region) <= 16
              - assert: AVG(qty) BETWEEN 0 AND 100
              - assert: COUNT(notes) >= 0.9 * ROW_COUNT()
              - assert: ROW_COUNT() > 0
              - assert: ANY(flag)
    "});
}

// A column constraint bareword must be one of the four structural names.
#[test]
fn constraints_column_unknown_bareword() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: string
                examples: [x]
                constraints:
                  - primary
    "});
    diagnostic.assert_contains(&["Q-1-12", "primary_key", r#"got '"primary"'"#]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A malformed column assertion map matches neither `anyOf` branch, so it falls
// back to the enum branch's message rather than a precise "missing assert".
#[test]
fn constraints_column_malformed_assertion() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: string
                examples: [x]
                constraints:
                  - description: missing the assert key
    "});
    diagnostic.assert_contains(&["Q-1-12", "primary_key"]);
}

// A table constraint must be an assertion map; a bareword is a plain string and
// is rejected as the wrong type.
#[test]
fn constraints_table_bareword_rejected() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: string
                examples: [x]
            constraints:
              - required
    "});
    diagnostic.assert_contains(&["Q-1-11", "Expected object, got string"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A table assertion map must carry `assert`.
#[test]
fn constraints_table_missing_assert() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: string
                examples: [x]
            constraints:
              - description: no assert here
    "});
    diagnostic.assert_contains(&["Q-1-10", "Missing required property 'assert'"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// Assertion maps are closed: an unknown key is rejected.
#[test]
fn constraints_table_unknown_property() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: string
                examples: [x]
            constraints:
              - assert: end_date >= start_date
                bogus: 1
    "});
    diagnostic.assert_contains(&["Q-1-18", "Unknown property 'bogus'"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// `assert` and `description` are both strings.
#[test]
fn constraints_table_assert_not_string() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: t
            columns:
              - name: a
                type: string
                examples: [x]
            constraints:
              - assert: 42
    "});
    diagnostic.assert_contains(&["Q-1-11", "Expected string"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// --- in-memory entry point ----------------------------------------------

#[test]
fn validate_spec_str_accepts_valid_content() {
    let content = indoc! {"
        $version: 0.1.0
        $learn_more: http://data-dict.tidyverse.org/
        tables:
          - name: t
            description: A table.
            source:
              parquet: t.parquet
            columns:
              - name: c
                type: string
                examples: [a, b]
                description: A column.
    "};
    let problems = data_dict::validate_spec_str(content, "buffer.yaml");
    assert!(!problems.status().failed());
}

#[test]
fn validate_spec_str_reports_located_schema_error() {
    // Missing the required `$version` key: a structural schema failure that
    // still resolves to a location in the buffer.
    let problems = data_dict::validate_spec_str("tables: {}\n", "buffer.yaml");
    assert!(problems.status().failed());
    let schema_error = problems
        .items
        .iter()
        .find(|p| p.code.is_some())
        .expect("expected a coded schema problem");
    assert!(schema_error.location(&problems.source).is_some());
}

// --- definitions ---------------------------------------------------------
//
// Definitions are checked inline (they fit in a few lines of YAML), against a
// small orders table with one numeric and one coded column.

// A filter, a metric, and a derived value with their full documentation, the
// metric and derived value building on the filter.
#[test]
fn definitions_ok() {
    assert_clean_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
              - name: status_cd
                type: number(id)
                examples: [10, 90]
            definitions:
              - name: is_returned
                label: Returned orders
                description: Orders that came back.
                details: Status 90 is the legacy return code.
                expr: status_cd = 90
              - name: net_revenue
                expr: SUM(CASE WHEN is_returned THEN 0 ELSE order_total END)
              - name: avg_order
                expr: net_revenue / row_count()
    "});
}

// A filter written with COLUMNS(...) over several columns.
#[test]
fn definition_columns_filter_ok() {
    assert_clean_dict(indoc! {"
        tables:
          - name: survey
            columns:
              - name: q1
                type: number(ordinal)
                range: [1, 5]
              - name: q2
                type: number(ordinal)
                range: [1, 5]
            definitions:
              - name: all_present
                expr: COLUMNS('q[1-2]') IS NOT NULL
    "});
}

#[test]
fn definition_duplicate_name() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: total
                expr: SUM(order_total)
              - name: total
                expr: SUM(order_total) * 2
    "});
    diagnostic.assert_contains(&["S10", "Definition names must be unique"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn definition_empty_name() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name:
                expr: SUM(order_total)
    "});
    diagnostic.assert_contains(&["S11", "the `name` is empty"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn definition_shadows_column() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: order_total
                expr: SUM(order_total)
    "});
    diagnostic.assert_contains(&["S33", "can't overlap", "order_total"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// The generic expression checks (S19–S22) apply to definitions unchanged;
// one minimal test each pins that they're wired up, with the assertion-side
// snapshots covering the exact rendering.
#[test]
fn definition_unparseable() {
    assert_invalid_dict(
        indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: total
                expr: 1 +
    "},
        &["S19", "does not parse"],
    );
}

#[test]
fn definition_unknown_name() {
    assert_invalid_dict(
        indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: total
                expr: SUM(order_totals)
    "},
        &["S20", "order_totals", "columns/definitions"],
    );
}

#[test]
fn definition_ill_typed() {
    assert_invalid_dict(
        indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: total_len
                expr: LENGTH(order_total)
    "},
        &["S21", "LENGTH"],
    );
}

// COLUMNS(...) is only meaningful in a filter: a metric over a selection has
// one value per selected column, not one per group.
#[test]
fn definition_columns_not_filter() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: survey
            columns:
              - name: q1
                type: number(ordinal)
                range: [1, 5]
              - name: q2
                type: number(ordinal)
                range: [1, 5]
            definitions:
              - name: total
                expr: SUM(COLUMNS('q[1-2]'))
    "});
    diagnostic.assert_contains(&["S21", "must be a boolean filter"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A metric referenced inside an aggregate is a nested aggregate (S30), just
// like a literal SUM(SUM(x)).
#[test]
fn definition_metric_in_aggregate() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: total
                expr: SUM(order_total)
              - name: double_counted
                expr: SUM(total)
    "});
    diagnostic.assert_contains(&["S30", "already an aggregate"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A definition referencing a broken definition inherits the permissive `Any`
// type, so the root cause is reported once, at the broken definition.
#[test]
fn definition_broken_reference_no_cascade() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: broken
                expr: LENGTH(order_total)
              - name: uses_broken
                expr: broken + 1
    "});
    diagnostic.assert_contains(&["S21", "LENGTH"]);
    assert_eq!(diagnostic.rendered.matches("S21").count(), 1);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A definition whose expression doesn't parse is S19 at its own location;
// definitions and assertions referencing it see the permissive `Any` type
// rather than a cascading S20.
#[test]
fn definition_unparseable_reference_no_cascade() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: broken
                expr: 1 +
              - name: uses_broken
                expr: broken + 1
            constraints:
              - assert: order_total <= uses_broken
    "});
    diagnostic.assert_contains(&["S19", "does not parse"]);
    assert!(!diagnostic.rendered.contains("S20"));
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn definition_cycle() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: a
                expr: b + 1
              - name: b
                expr: a + 1
    "});
    diagnostic.assert_contains(&["S34", "cycle"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

#[test]
fn definition_self_reference() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: c
                expr: c + 1
    "});
    diagnostic.assert_contains(&["S34", "c → c"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A definition may depend on a definition that depends on a cycle: the cycle
// is reported, and the downstream definition still checks cleanly.
#[test]
fn definition_downstream_of_cycle() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: a
                expr: b + 1
              - name: b
                expr: a + 1
              - name: downstream
                expr: a * 2
    "});
    diagnostic.assert_contains(&["S34"]);
    assert!(!diagnostic.rendered.contains("downstream"));
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A COLUMNS(...) regex matching nothing is a warning in a definition too.
#[test]
fn definition_empty_column_selection() {
    warning_dict(indoc! {"
        tables:
          - name: survey
            columns:
              - name: q1
                type: number(ordinal)
                range: [1, 5]
            definitions:
              - name: all_present
                expr: COLUMNS('zzz') IS NOT NULL
    "})
    .assert_contains(&["S22", "matches no columns"]);
}

// --- assertions referencing definitions ------------------------------------

// A column and a table assertion building on a named filter and a named
// metric.
#[test]
fn assertion_using_definitions_ok() {
    assert_clean_dict(indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
              - name: status_cd
                type: number(id)
                examples: [10, 90]
                constraints:
                  - assert: NOT(is_returned) OR order_total > 0
            definitions:
              - name: is_returned
                expr: status_cd = 90
              - name: avg_order
                expr: SUM(order_total) / row_count()
            constraints:
              - assert: order_total <= 2 * avg_order
    "});
}

// A filter definition's COLUMNS(...) is the assertion's one allowed selection
// once the reference is substituted in.
#[test]
fn assertion_using_columns_filter_ok() {
    assert_clean_dict(indoc! {"
        tables:
          - name: survey
            columns:
              - name: q1
                type: number(ordinal)
                range: [1, 5]
              - name: q2
                type: number(ordinal)
                range: [1, 5]
            definitions:
              - name: all_present
                expr: COLUMNS('q[1-2]') IS NOT NULL
            constraints:
              - assert: all_present
    "});
}

// The at-most-one COLUMNS(...) rule binds after definition references are
// substituted in: the assertion's own selection and the filter's combine to
// two, which evaluation couldn't resolve to a single selection.
#[test]
fn assertion_plus_filter_two_columns_selections() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: survey
            columns:
              - name: q1
                type: number(ordinal)
                range: [1, 5]
              - name: q2
                type: number(ordinal)
                range: [1, 5]
            definitions:
              - name: all_present
                expr: COLUMNS('q[1-2]') IS NOT NULL
            constraints:
              - assert: COLUMNS(*) IS NOT NULL AND all_present
    "});
    diagnostic.assert_contains(&["S21", "at most one `COLUMNS(...)`", "recursively includes"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A selection that arrives through a reference still makes the definition a
// filter: building a non-boolean value on one is S21.
#[test]
fn definition_non_filter_via_reference() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: survey
            columns:
              - name: q1
                type: number(ordinal)
                range: [1, 5]
              - name: q2
                type: number(ordinal)
                range: [1, 5]
            definitions:
              - name: filt
                expr: COLUMNS(*) > 0
              - name: bad
                expr: CASE WHEN filt THEN 1 ELSE 0 END
    "});
    diagnostic.assert_contains(&["S21", "must be a boolean filter", "recursively includes"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A definition's own selection plus one brought in by a reference is two
// selections: over the budget of one.
#[test]
fn definition_own_selection_plus_reference() {
    let diagnostic = failing_dict(indoc! {"
        tables:
          - name: survey
            columns:
              - name: q1
                type: number(ordinal)
                range: [1, 5]
              - name: q2
                type: number(ordinal)
                range: [1, 5]
            definitions:
              - name: f1
                expr: COLUMNS('q1') IS NOT NULL
              - name: f2
                expr: f1 AND COLUMNS('q2') IS NOT NULL
    "});
    diagnostic.assert_contains(&["S21", "at most one `COLUMNS(...)`", "recursively includes"]);
    #[cfg(unix)]
    assert_snapshot!(diagnostic);
}

// A boolean definition building on a filter is itself a filter; the single
// selection travels with it.
#[test]
fn definition_filter_via_reference_ok() {
    assert_clean_dict(indoc! {"
        tables:
          - name: survey
            columns:
              - name: q1
                type: number(ordinal)
                range: [1, 5]
              - name: q2
                type: number(ordinal)
                range: [1, 5]
            definitions:
              - name: all_present
                expr: COLUMNS('q[1-2]') IS NOT NULL
              - name: any_missing
                expr: NOT(all_present)
    "});
}

// An assertion referencing a name that is neither a column nor a definition.
#[test]
fn assertion_unknown_name_with_definitions() {
    assert_invalid_dict(
        indoc! {"
        tables:
          - name: orders
            columns:
              - name: order_total
                type: number(quantity)
                range: [0, 10000]
            definitions:
              - name: total
                expr: SUM(order_total)
            constraints:
              - assert: order_total <= 2 * totl
    "},
        &["S20", "totl", "columns/definitions"],
    );
}
