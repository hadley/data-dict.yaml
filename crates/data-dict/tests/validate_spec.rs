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
    let path = dict(body);
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
    diagnostic.assert_contains(&["S20", "`b`", "not on this table"]);
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
