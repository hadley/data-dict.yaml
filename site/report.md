# Report

A **report** is one validation run's findings as JSON, so a program can act on them. Every check in [validation.md](validation.md) (`S##`, `M##`, or `D##`) reports through this one document, and only those checks do. The `data-dict` CLI writes one for any validation run it can get started.

A report is a superset of the diagnostics rendered for a person: every position a diagnostic highlights is in it, and it names more offending rows.

The [level](validation.md#three-levels-of-validation) validated determines which checks ran, not the shape of what they report: a data-level report carries `S##` and `M##` problems too, since each level implies the ones before it. `D##` problems do carry something the earlier levels have no use for: found by reading the data, they name the offending rows, not just the table and column the dictionary declares. If an earlier level finds an error, the run stops there and reports only what it got to. A run that finds nothing still produces a report, with an empty `problems` list.

A failure that stops the run before any check can be applied is not a finding about the data dictionary and has no code, so it is not reported this way at all: the file can't be read or isn't YAML, a Parquet file fails mid-read, or the table asked for isn't in the dictionary. The CLI reports such a failure as a plain error and writes no report. Every problem in a report therefore carries a `code`.

## Output shape

A key with nothing to say is **omitted** rather than serialized as `null` or `[]`: keys marked `?` below may be absent, meaning the value doesn't apply to this problem or step. Zeroes and falses are real data and always appear. Consumers should read absent and null interchangeably.

The problems are one flat list. They are deliberately not also grouped by check, by table, or by row: everything needed to build those views is on each problem, and a consumer that wants them can group the list itself rather than reconcile several copies of the same finding.

### Top level

```jsonc
{
  "$version": "0.1.0",           // version of the report document format itself
  "status": "ok" | "warning" | "error",
  "steps": [ Step ],
  "problems": [ Problem ]
}
```

`status` is the worst severity present: `error` if any problem is an error, `warning` if there are only warnings, `ok` if there are none.

`steps` is what the run checked, `problems` is what it found. A step is one check applied to one target: a column, a set of key columns, an assertion, or a table. Every step the run reached is listed whether it passed or failed, so a consumer can report a pass rate and not just a failure list.

Which steps exist follows from the dictionary: a `required` column gets a `D01` step, a `unique` one a `D02` step, each `assert` a `D07` step, and a column with no constraint gets none. A spec check (`S##`) reads the document as a whole rather than one declared target, so it is reported through `problems` alone, and `steps` is empty for a spec-level run.

Every step the level attempted is listed, including the ones it could not weigh. A step is missing only when its level never ran at all: a spec error stops the run, so the metadata and data steps that would have followed are absent rather than listed unevaluated.

### Problem

```jsonc
{
  "code": "D04",                 // the check's code in validation.md
  "step?": 3,                    // the `id` of the step that found it
  "severity": "error" | "warning",
  "kind": "values_outside_enum", // the finding's shape, see below
  "message": "string",           // what was found, a lowercase fragment
  "expected?": "string",         // what the spec requires, a full sentence
  "hint?": "string",             // advice on how to fix it
  "suggestion?": { "title": "string", "replacement": "string",
                   "location": Location },
  "table?": "string",            // the table the problem is about
  "columns?": ["string"],        // the columns it is about, dotted for a struct field
  "location?": Location,         // the offending node itself
  "context?": [ Location ],      // the nodes enclosing it, outermost first
  ...                            // the keys of this `kind`, see below
}
```

`expected` and `message` are two halves of one sentence: `expected` states the rule in general ("A range's minimum must be less than or equal to its maximum."), `message` reports the specific violation ("minimum `100` is greater than maximum `10`"). A problem always has a `message`; it has an `expected` whenever the rule can be stated in the abstract.

`columns` is every column the problem is about, in dictionary order: one entry for a problem about a single column, several for a composite key or an assertion that reads more than one (`start_date < end_date` names both). It is absent for a problem about no column in particular — a table-level or document-level one. A consumer that groups by column lists such a problem under each of its columns.

`suggestion` is a concrete edit: splice `replacement` into the source over the suggestion's own `location`, which is an insertion point when it is empty.

`step` is absent for a problem no step accounts for: a spec problem, and an undocumented column (`M03`). A step whose `outcome` is `fail` always has at least one problem pointing back at it.

### Step

```jsonc
{
  "id": 3,                       // 1-based, unique within this report
  "code": "D04",                 // the check's code in validation.md
  "table": "otters",             // the table the step checked
  "columns?": ["site", "day"],   // the columns it checked, dotted for a struct field
  "assertion?": "weight > 0",    // the expression, for an assertion step
  "outcome": "pass" | "fail" | "unevaluated",
  "row_count?": 91043,           // rows of the table the step covered
  "failed_row_count?": 2         // rows that failed; passed is the difference
}
```

`columns` follows the same rule as a problem's: every column the step checked, in dictionary order — one for a per-column check, the key's columns for a composite key, and every column the expression reads for an assertion. It is absent for a step about the table as a whole (`M04`).

Steps are in dictionary order: tables as `tables` declares them, then each table's columns in order, then that column's checks by code. A step over several columns sorts by the first of them.

A step's `code` is the code it reports when the thing it checks is plainly wrong. Several checks are alternative verdicts on one declared target — a column is either the wrong type (`M01`) or absent (`M02`), and a uniqueness check either finds duplicates (`D02`) or can't compare the values at all (`D03`) — so one step covers them and the problem carries the code that actually applied:

| Step `code` | One step per | Problem codes it can report |
|-------------|--------------|-----------------------------|
| `M01` | column the dictionary declares | `M01`, `M02` |
| `M04` | table validated against data | `M04`, `M05` |
| `D01` | `required` or `primary_key` column | `D01` |
| `D02` | `unique` column, and the primary key as a whole | `D02`, `D03` |
| `D04` | `enum` column, including a nested one | `D04` |
| `D05` | single-column `foreign_key` | `D05`, `D06` |
| `D07` | `assert` expression | `D07`, `D08`, `D09` |

: {tbl-colwidths="[15,40,45]"}

`M03` is the one check with nothing to declare it: the column exists only in the data, so no step covers it and its problem has no `step`.

`outcome` is the step's verdict, and the only thing a consumer should read it from: `pass` if the step found nothing, `fail` if at least one problem points back at it, `unevaluated` if it could not reach a verdict at all. Do not infer the verdict from the counts below — a step over an empty table fails with nothing to count.

`outcome` is `unevaluated` when the values were not comparable (`D03`, `D06`), the expression could not be run (`D08`, `D09`), or the table's data could not be read, which leaves every step of that table unevaluated. A step that did not evaluate has not passed, and a consumer must not count it as one.

`row_count` is how many of the table's rows the step covered and `failed_row_count` how many of them failed, so every step of a table counts in the same unit against the same denominator. A check with a single verdict is all or nothing: a metadata step, or an aggregate assertion like `SUM(weight) > 0`, fails every row of the table or none of them. That is a coarse weighting so such a step can be counted alongside the row-level ones, not a claim about which rows are at fault — an aggregate assertion's own problem still blames no individual row.

Both keys are absent when the step has no row count to report: when `outcome` is `unevaluated`, and when the failure is that the table's rows could never be counted in the first place (`M04`, `M05`).

A step carries no values, only counts, so nothing on it is ever withheld for a [restricted](spec.md#display) column.

Nothing is duplicated between the two lists: a step says how much was checked and how much failed, and its problems say what failed and where. A step-by-step report table is `steps` joined to `problems` on `id`, and that join is the consumer's to make, for the same reason `problems` is not also grouped by table or by code.

### Location

A `Location` is a span of the `data-dict.yaml` file — never of the data. Even a `D##` problem locates itself in the dictionary, at the place where the dictionary makes the claim the data broke; the data side is reported as row numbers instead.

```jsonc
{ "start_line": 0, "start_column": 2, "end_line": 0, "end_column": 9 }
```

Lines and columns count from 0, following the LSP convention. Diagnostics rendered for a person show the same positions 1-based.

`location` is the offending node itself, and `context` the nodes enclosing it — for a bad value in a column, the table's name and the column's name. Together they are everything a rendered diagnostic highlights, so a consumer can draw the same annotated excerpt from the document alone. Both are absent for a problem with no place in the file — a metadata problem about a column the dictionary never mentions (`M03`).

### Kinds

`kind` names the shape of the finding, and decides which further keys the problem carries. Each kind maps to one check, except `schema` and `spec`, which cover every structural and every semantic spec check respectively; consult `validation.md` for what a check means.

| `kind` | `code` | Additional keys |
|--------|--------|-----------------|
| `schema` | `S60`–`S69` | |
| `spec` | `S01`–`S31` | |
| `type_mismatch` | `M01` | `declared`, `actual` |
| `missing_in_data` | `M02` | |
| `extra_in_data` | `M03` | `actual` |
| `missing_source` | `M04` | |
| `unreadable_source` | `M05` | |
| `nulls_in_required` | `D01` | `count`, `rows` |
| `duplicate_values` | `D02` | `count`, `rows`, `values`, `redacted` |
| `uniqueness_not_verified` | `D03` | `reason` |
| `values_outside_enum` | `D04` | `count`, `rows`, `values`, `redacted` |
| `foreign_key_not_found` | `D05` | `column`, `references`, `count`, `rows`, `values`, `redacted` |
| `referential_integrity_not_verified` | `D06` | `column`, `references`, `reason` |
| `assertion_violated` | `D07` | `assertion`, `count`, `rows`, `values`, `redacted` |
| `assertion_false` | `D07` | `assertion` |
| `assertion_not_checked` | `D08` | `assertion`, `column`, `reason` |
| `assertion_overflow` | `D09` | `assertion`, `row` |

: {tbl-colwidths="[30,10,60]"}

`schema` and `spec` are the two halves of spec validation and are kept apart because a consumer treats them differently: a `schema` problem means the document could not be read as a data dictionary at all, so no `spec` problem could be looked for, while a `spec` problem is a finding about a document that was read successfully.

`column`, singular, is the one column a kind singles out, as against the list in `columns`: `D05` and `D06` put the referencing column there and name the target they compare it against in `references`, and `D08`'s is the one column of the assertion that can't be read as its declared type — absent when the obstacle isn't one column's type. `D02` and `D03` single out no column, and carry the key's columns in `columns` alone.

`values_outside_enum` and `assertion_violated` also arise for a nested column: `columns` then holds the dotted path to the struct field, and its rows are the rows of the top-level column that holds it.

## Counting and capping

Three keys describe how many rows broke a check and which ones:

* `count` is the **exact** total number of offending rows. It is never capped.
* `rows` are the offending row numbers, ascending, **capped** at the first so many. Row numbers are 1-based and absolute within the table's Parquet file, so they survive row-group boundaries and can be used to seek back into the data.
* `values` are the offending values themselves, one **object** per offending row, keyed by column name.

A `values` entry holds just the columns the check is about — the enum column for `D04`, the referencing column for `D05`, the key's columns for `D02`, the columns the expression reads for `D07` — never the whole row. Entries line up with `rows`: one per row number, in the same order, and nothing is deduplicated, so a value that offends twice appears twice.

```jsonc
"rows": [57, 812, 4310],
"values": [{"status": "retired"}, {"status": "unknown"}, {"status": "retired"}]
```

```jsonc
"rows": [12, 91],
"values": [{"weight": "-1.5", "length": "60"}, {"weight": "0", "length": "74"}]
```

An offending value is always a **string**, and never a JSON number or boolean. Each value renders the way the same value would be written as a `range` bound or an `examples` entry in the dictionary itself: a number in decimal at full precision, a date or datetime as ISO 8601, a boolean as `"true"` or `"false"`, a non-finite float as `"NaN"`, `"Infinity"`, or `"-Infinity"`.

A value that is *missing* is the one thing that isn't a string: it is JSON `null`, so it stays distinct from a string column that really holds `"null"`. It can only appear in a `D07` entry — `D02`, `D04`, and `D05` all exempt nulls, so their `values` never contain one.

`rows` and `values` are each capped, so that a wholly broken column can't turn a report into megabytes of row numbers. The cap is the producer's to choose — the `data-dict` CLI reports the first 5 — and a producer may offer a way to raise or lower it. A cap truncates: what's reported is the first so many in the order the key defines, never a selection drawn from across the table. `count` is never capped, so `count > rows.length` means the list was truncated. The converse doesn't hold — some checks report a count with no rows at all.

### What each check can say

A check reports what its evidence supports, and the evidence differs. These are properties of the checks themselves, not gaps to be worked around:

* `D01` and `D02` can sometimes be settled from Parquet footer statistics alone, without reading a single value. That path proves how many rows offend but not which, so `count` is present and `rows` and `values` are empty.
* `D02`'s `rows` are the rows of the **repeat** occurrences — the row that first held a value is not the one reported as a duplicate — and its `values` follow them, so a key duplicated twice appears twice. A reported value is the key **as stored**, not the form the comparison normalizes it to, so `-0.0` reads as it was written even though it duplicates `0.0`, and two `values` entries that duplicate each other can read differently.
* `D07` on an aggregate assertion (`SUM(x) > 0`) reports `assertion_false`, with no `count` and no `rows`. An aggregate is a single verdict about the whole table, so no row is to blame.
* `D09` reports a single `row`: evaluation stops at the first overflow, since everything after it is computed from a value that already went wrong.
* `D03`, `D06`, and `D08` report that a check *didn't run*, and carry a `reason` naming the barrier rather than any row.

## Restricted columns

A column marked [`display: restricted`](spec.md#display) holds data that must not be surfaced by default. A validation report is user-facing output like any other, and one it is easy to forget: a CI job that archives its reports would otherwise leave restricted values sitting in a build artifact.

So a problem about a restricted column — or about an assertion that reads one — reports `count` and `rows` but no `values`, and sets `"redacted": true`. The rows are still there, so a consumer can still find the offending records in the data it is already entitled to read; only the values are withheld.

Redaction is per column: a restricted column's key is left out of each entry, while its unrestricted neighbours keep their values. If every column the entry would name is restricted — a restricted `enum` or `unique` column, or an assertion that reads nothing else — there is nothing left to say, so `values` is omitted rather than given as a list of empty objects.

`redacted` is always present on the kinds that can carry values, so `"redacted": false` positively states that nothing was withheld.

[Withholding](validation.md#reporting) is a property of validation itself rather than of this format: a restricted column's values are never reported, in a report or in a diagnostic.

## Example

A data-level run over a two-table dictionary, with an unreadable source, a duplicated key, a bad enum value in a restricted column, and a violated assertion:

```jsonc
{
  "$version": "0.1.0",
  "status": "error",
  "steps": [
    // an M01 step per column also ran and passed; elided here
    {"id": 1, "code": "M04", "table": "otters",
     "outcome": "pass", "row_count": 91043, "failed_row_count": 0},
    {"id": 2, "code": "D01", "table": "otters", "columns": ["id"],
     "outcome": "pass", "row_count": 91043, "failed_row_count": 0},
    {"id": 3, "code": "D02", "table": "otters", "columns": ["id"],
     "outcome": "fail", "row_count": 91043, "failed_row_count": 3},
    {"id": 4, "code": "D04", "table": "otters", "columns": ["carer_email"],
     "outcome": "fail", "row_count": 91043, "failed_row_count": 2},
    {"id": 5, "code": "D07", "table": "otters", "columns": ["weight"], "assertion": "weight > 0",
     "outcome": "fail", "row_count": 91043, "failed_row_count": 2},
    // the source couldn't be read, so `sightings` has no row count either
    {"id": 6, "code": "M04", "table": "sightings", "outcome": "fail"},
    // and no step of `sightings` reached a verdict
    {"id": 7, "code": "D01", "table": "sightings", "columns": ["otter_id"], "outcome": "unevaluated"}
  ],
  "problems": [
    {
      "code": "M05",
      "step": 6,
      "severity": "error",
      "kind": "unreadable_source",
      "expected": "A table's `source` must point at a readable Parquet file.",
      "message": "No such file or directory (os error 2)",
      "table": "sightings",
      // the `parquet:` value, inside the table that declares it
      "location": {"start_line": 22, "start_column": 14, "end_line": 22, "end_column": 31},
      "context": [{"start_line": 20, "start_column": 4, "end_line": 20, "end_column": 13}]
    },
    {
      "code": "D02",
      "step": 3,
      "severity": "error",
      "kind": "duplicate_values",
      "expected": "A unique column must not contain duplicate values.",
      "message": "has 3 repeated occurrences (rows: 118, 4092, 91043)",
      "table": "otters",
      "columns": ["id"],
      "count": 3,
      "rows": [118, 4092, 91043],
      "values": [{"id": "L-0042"}, {"id": "L-0042"}, {"id": "L-1177"}],
      "redacted": false,
      // the `unique` constraint, inside the column, inside the table
      "location": {"start_line": 9, "start_column": 21, "end_line": 9, "end_column": 27},
      "context": [{"start_line": 3, "start_column": 4, "end_line": 3, "end_column": 10},
                  {"start_line": 8, "start_column": 10, "end_line": 8, "end_column": 12}]
    },
    {
      "code": "D04",
      "step": 4,
      "severity": "error",
      "kind": "values_outside_enum",
      "expected": "An enum column's values must all be among its declared `values`.",
      "message": "has 2 values outside the allowed set (values withheld; rows: 57, 812)",
      "table": "otters",
      "columns": ["carer_email"],
      "count": 2,
      "rows": [57, 812],
      "redacted": true,
      "location": {"start_line": 15, "start_column": 12, "end_line": 15, "end_column": 40},
      "context": [{"start_line": 3, "start_column": 4, "end_line": 3, "end_column": 10},
                  {"start_line": 14, "start_column": 10, "end_line": 14, "end_column": 21}]
    },
    {
      "code": "D07",
      "step": 5,
      "severity": "error",
      "kind": "assertion_violated",
      "expected": "An assertion must hold for every row.",
      "message": "is false for 2 rows: 12, 91 (weight=-1.5)",
      "table": "otters",
      "columns": ["weight"],
      "assertion": "weight > 0",
      "count": 2,
      "rows": [12, 91],
      "values": [{"weight": "-1.5"}, {"weight": "0"}],
      "redacted": false,
      "location": {"start_line": 19, "start_column": 8, "end_line": 19, "end_column": 18},
      "context": [{"start_line": 3, "start_column": 4, "end_line": 3, "end_column": 10},
                  {"start_line": 17, "start_column": 10, "end_line": 17, "end_column": 16}]
    }
  ]
}
```

Grouping that list by `table`, by `column`, or by `code` is a one-liner in any language; here it is in `jq`, collecting every offending row per table:

```bash
jq '.problems | group_by(.table)
   | map({table: .[0].table, rows: (map(.rows // []) | add | unique)})' report.json
```

And a step-by-step table, one row per check with its pass and fail counts:

```bash
jq '.steps | map({step: .id, code, table, columns, outcome, row_count,
                  passed: (if .row_count then .row_count - .failed_row_count else null end),
                  failed_row_count})' report.json
```
