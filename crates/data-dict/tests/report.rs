//! Integration tests for the report document (`site/report.md`): what a run
//! says it checked, not just what it found.
//!
//! Each test snapshots the whole report as JSON, so the steps and the problems
//! are read together — a step's outcome only makes sense beside the problem
//! that failed it.

mod common;
use common::{temp_dir, write_dict, write_parquet};

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use data_dict::{Level, ProblemSet, Run, validate_data, validate_meta};
use indoc::indoc;

/// The report as pretty JSON, ready to snapshot. The run is stamped by hand:
/// the real dictionary sits in a temp directory, the clock moves, and the
/// version changes on release, none of which a snapshot can hold.
fn report(problems: &ProblemSet, level: Level) -> String {
    let mut run = Run::new(Path::new("data-dict.yaml"), level, None);
    run.generated_at = "2026-01-01T00:00:00Z".to_string();
    run.tool.version = Some("0.0.0");
    serde_json::to_string_pretty(&problems.report(run)).unwrap()
}

/// Write a dictionary next to the two-column parquet file `write_parquet`
/// produces (`name`, a string, and `weight`, a double; two rows).
fn otters(body: &str) -> std::path::PathBuf {
    let dir = temp_dir();
    write_parquet(&dir.join("otters.parquet"));
    write_dict(&dir, body)
}

/// Snapshot a report, hiding the redundant `expression:` header. Snapshots run
/// on Unix alone, so the macro is unused elsewhere.
#[allow(unused_macros)]
macro_rules! assert_report {
    ($problems:expr, $level:expr) => {{
        let body = report(&$problems, $level);
        let mut settings = insta::Settings::clone_current();
        settings.set_omit_expression(true);
        let _guard = settings.bind_to_scope();
        insta::assert_snapshot!(body);
    }};
}

const CLEAN: &str = indoc! {"
    tables:
      - name: otters
        source:
          parquet: otters.parquet
        columns:
          - name: name
            type: string
            constraints: [primary_key]
            examples: [otter, seal]
          - name: weight
            type: number
            examples: [1.0, 2.0]
            constraints:
              - assert: weight > 0
"};

#[test]
fn a_clean_run_lists_every_step_as_passed() {
    let dict = otters(CLEAN);
    let problems = validate_data(&dict, None);
    assert!(problems.items.is_empty(), "{:?}", problems.items);
    #[cfg(unix)]
    assert_report!(problems, Level::Data);
}

/// An assertion the author described lists that description on its step, so
/// the report can name the check in the author's words rather than the
/// expression's.
#[test]
fn an_assertion_step_carries_its_authors_description() {
    let dict = otters(indoc! {"
        tables:
          - name: otters
            source:
              parquet: otters.parquet
            columns:
              - name: name
                type: string
                examples: [otter, seal]
              - name: weight
                type: number
                examples: [1.0, 2.0]
                constraints:
                  - assert: weight > 0
                    description: An otter always weighs something.
    "});
    let problems = validate_data(&dict, None);
    let step = last_step(&problems, "D07");
    assert_eq!(
        step.description.as_deref(),
        Some("An otter always weighs something.")
    );
    #[cfg(unix)]
    assert_report!(problems, Level::Data);
}

/// A metadata-level run checks only names and types, so it registers only the
/// steps it runs: no `D##` step is listed as passed that never ran.
#[test]
fn a_metadata_run_registers_only_metadata_steps() {
    let dict = otters(CLEAN);
    let problems = validate_meta(&dict, None);
    let codes: Vec<&str> = problems.steps.items().iter().map(|s| s.code).collect();
    assert_eq!(codes, ["M04", "M01", "M01"]);
    #[cfg(unix)]
    assert_report!(problems, Level::Meta);
}

/// A spec check reads the document as a whole rather than any declared target,
/// so a spec-level run has nothing to list.
#[test]
fn a_spec_run_has_no_steps() {
    let dict = otters(CLEAN);
    let problems = data_dict::validate_spec(&dict);
    assert!(problems.steps.items().is_empty());
}

/// A report locates its problems but carries no copy of the text, so a consumer
/// drawing its own excerpt needs the document the spans were measured against.
#[test]
fn a_run_can_hand_back_the_text_its_spans_were_measured_against() {
    let dict = otters(CLEAN);
    let problems = validate_data(&dict, None);
    assert_eq!(
        problems.source_text(),
        Some(std::fs::read_to_string(&dict).unwrap().as_str())
    );
}

/// A run that never read a document has no text to hand back.
#[test]
fn a_preflight_failure_has_no_text() {
    let dir = temp_dir();
    let problems = validate_data(&dir.join("absent.yaml"), None);
    assert!(problems.preflight().is_some());
    assert_eq!(problems.source_text(), None);
}

#[test]
fn an_unreadable_source_leaves_the_tables_other_steps_unevaluated() {
    let dir = temp_dir();
    let dict = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: otters
                source:
                  parquet: absent.parquet
                columns:
                  - name: name
                    type: string
                    constraints: [primary_key]
                    examples: [otter, seal]
        "},
    );
    let problems = validate_data(&dict, None);
    let steps = problems.steps.items();
    assert_eq!(steps[0].code, "M04");
    assert_eq!(
        steps[0].outcome,
        data_dict::StepOutcome::Fail,
        "the source is the failure"
    );
    assert!(steps[0].row_count.is_none(), "no rows were ever counted");
    assert!(
        steps[1..]
            .iter()
            .all(|step| step.outcome == data_dict::StepOutcome::Unevaluated),
        "nothing else could be weighed: {steps:?}"
    );
}

#[test]
fn a_failing_check_weighs_its_step_by_the_rows_it_blames() {
    let dict = otters(indoc! {"
        tables:
          - name: otters
            source:
              parquet: otters.parquet
            columns:
              - name: name
                type: enum
                values: [otter, walrus]
              - name: weight
                type: number
                examples: [1.0, 2.0]
                constraints:
                  - assert: weight > 1.5
    "});
    let problems = validate_data(&dict, None);
    // `seal` is outside the enum, and one of the two weights is 1.0.
    let failed: Vec<_> = problems
        .steps
        .items()
        .iter()
        .filter(|step| step.outcome == data_dict::StepOutcome::Fail)
        .map(|step| (step.code, step.row_count, step.failed_row_count))
        .collect();
    assert_eq!(
        failed,
        [("D04", Some(2), Some(1)), ("D07", Some(2), Some(1))]
    );
    #[cfg(unix)]
    assert_report!(problems, Level::Data);
}

/// An aggregate assertion is one verdict about the whole table, so it blames
/// every row rather than any of them.
#[test]
fn an_aggregate_assertion_blames_every_row() {
    let dict = otters(indoc! {"
        tables:
          - name: otters
            source:
              parquet: otters.parquet
            columns:
              - name: name
                type: string
                examples: [otter, seal]
              - name: weight
                type: number
                examples: [1.0, 2.0]
            constraints:
              - assert: SUM(weight) > 100
    "});
    let problems = validate_data(&dict, None);
    let step = last_step(&problems, "D07");
    assert_eq!(step.outcome, data_dict::StepOutcome::Fail);
    assert_eq!((step.row_count, step.failed_row_count), (Some(2), Some(2)));
    assert!(
        matches!(
            problems.items[0].kind,
            data_dict::ProblemKind::AssertionFalse { .. }
        ),
        "an aggregate blames no individual row: {:?}",
        problems.items[0].kind
    );
    assert_eq!(problems.items[0].columns, ["weight"]);
}

/// A `list(enum)` column holds several values per row, so the values that broke
/// the check and the rows that did are different numbers: the problem counts
/// values, the step counts rows.
#[test]
fn a_list_column_fails_fewer_rows_than_values() {
    use arrow_array::builder::{ListBuilder, StringBuilder};
    use arrow_array::{ArrayRef, RecordBatch};
    use parquet::arrow::ArrowWriter;

    let dir = temp_dir();
    // Row 1 holds two values outside the enum; row 2 holds none.
    let mut tags = ListBuilder::new(StringBuilder::new());
    for row in [["x", "y"], ["a", "a"]] {
        for value in row {
            tags.values().append_value(value);
        }
        tags.append(true);
    }
    let batch =
        RecordBatch::try_from_iter(vec![("tags", Arc::new(tags.finish()) as ArrayRef)]).unwrap();
    let file = File::create(dir.join("otters.parquet")).unwrap();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    let dict = write_dict(
        &dir,
        indoc! {"
            tables:
              - name: otters
                source:
                  parquet: otters.parquet
                columns:
                  - name: tags
                    type: list(enum)
                    values: [a]
        "},
    );
    let problems = validate_data(&dict, None);
    let data_dict::ProblemKind::ValuesOutsideEnum { count, .. } = problems.items[0].kind else {
        panic!("expected D04, got {:?}", problems.items[0].kind);
    };
    assert_eq!(count, 2, "two values broke the check");
    let step = last_step(&problems, "D04");
    assert_eq!(
        (step.row_count, step.failed_row_count),
        (Some(2), Some(1)),
        "but only one row did"
    );
}

/// A restricted column's values are withheld from the report, but its counts —
/// and its step — are not.
#[test]
fn a_restricted_column_reports_counts_but_no_values() {
    let dict = otters(indoc! {"
        tables:
          - name: otters
            source:
              parquet: otters.parquet
            columns:
              - name: name
                type: enum
                display: restricted
                values: [otter, walrus]
              - name: weight
                type: number
                examples: [1.0, 2.0]
    "});
    let problems = validate_data(&dict, None);
    let json = report(&problems, Level::Data);
    assert!(json.contains("\"redacted\": true"), "{json}");
    assert!(
        !json.contains("seal"),
        "the offending value must not appear"
    );
    let step = last_step(&problems, "D04");
    assert_eq!((step.row_count, step.failed_row_count), (Some(2), Some(1)));
}

/// A column the data doesn't have can't be checked, so the checks the
/// dictionary declares for it are listed as unevaluated rather than passed.
#[test]
fn a_column_missing_from_the_data_leaves_its_checks_unevaluated() {
    let dict = otters(indoc! {"
        tables:
          - name: otters
            source:
              parquet: otters.parquet
            columns:
              - name: name
                type: string
                examples: [otter, seal]
              - name: weight
                type: number
                examples: [1.0, 2.0]
              - name: age
                type: number
                examples: [3, 4]
                constraints: [required, unique]
    "});
    let problems = validate_data(&dict, None);
    let outcomes: Vec<_> = problems
        .steps
        .items()
        .iter()
        .filter(|step| step.columns == ["age"])
        .map(|step| (step.code, step.outcome))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("M01", data_dict::StepOutcome::Fail),
            ("D01", data_dict::StepOutcome::Unevaluated),
            ("D02", data_dict::StepOutcome::Unevaluated),
        ]
    );
}

/// Every problem a step found points back at it, and every problem a step
/// didn't find (`M03`, a spec problem) points at nothing.
#[test]
fn problems_point_back_at_the_step_that_found_them() {
    let dict = otters(indoc! {"
        tables:
          - name: otters
            source:
              parquet: otters.parquet
            columns:
              - name: name
                type: string
                examples: [otter, seal]
    "});
    let problems = validate_data(&dict, None);
    let m03 = problems
        .items
        .iter()
        .find(|p| p.code == Some("M03"))
        .expect("`weight` is in the data but not the dictionary");
    assert_eq!(m03.table.as_deref(), Some("otters"));
    assert_eq!(m03.columns, ["weight"]);
    assert!(
        m03.step.is_none(),
        "no step declares an undocumented column"
    );
}

fn last_step<'a>(problems: &'a ProblemSet, code: &str) -> &'a data_dict::Step {
    problems
        .steps
        .items()
        .iter()
        .find(|step| step.code == code)
        .unwrap_or_else(|| panic!("no {code} step in {:?}", problems.steps.items()))
}

/// The report is written from the dictionary, so a dictionary that can't be
/// read produces no report at all.
#[test]
fn a_preflight_failure_produces_no_report() {
    let dir = temp_dir();
    let problems = validate_data(&dir.join("absent.yaml"), None);
    assert!(problems.preflight().is_some());
    assert!(problems.steps.items().is_empty());
}
