# Report

A **report** is one validation run's findings as JSON, so a program can act on them. Every check that has a code in [validation.md](validation.md) (`S##`, `M##`, or `D##`) reports through this one document, as does a failure that stopped the run before any check could start. The `data-dict` CLI writes one for any validation run.

A report is a superset of the diagnostics rendered for a person: every position a diagnostic highlights is in it, and it names more offending rows.

The [level](validation.md#three-levels-of-validation) validated determines which checks ran, not the shape of what they report: a data-level report carries `S##` and `M##` problems too, since each level implies the ones before it. If an earlier level finds an error, the run stops there and reports only what it got to. A run that finds nothing still produces a report, with an empty `problems` list.

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
  "code?": "D04",                // the check's code in validation.md
  "step?": 3,                    // the `id` of the step that found it
  "severity": "error" | "warning",
  "kind": "values_outside_enum", // the finding's shape, see below
  "message": "string",           // what was found, a lowercase fragment
  "expected?": "string",         // what the spec requires, a full sentence
  "hint?": "string",             // advice on how to fix it
  "suggestion?": { "title": "string", "replacement": "string",
                   "location": Location },
  "table?": "string",            // the table the problem is about
  "column?": "string",           // the column, dotted for a struct field
  "location?": Location,         // the offending node itself
  "context?": [ Location ],      // the nodes enclosing it, outermost first
  ...                            // the keys of this `kind`, see below
}
```

`expected` and `message` are two halves of one sentence: `expected` states the rule in general ("A range's minimum must be less than or equal to its maximum."), `message` reports the specific violation ("minimum `100` is greater than maximum `10`"). A problem always has a `message`; it has an `expected` whenever the rule can be stated in the abstract.

`code` is absent only for a pre-flight failure — one that stopped the run before any check could run, like an unreadable file.

`suggestion` is a concrete edit: splice `replacement` into the source over the suggestion's own `location`, which is an insertion point when it is empty.

`step` is absent for a problem no step accounts for: a pre-flight failure, a spec problem, and an undocumented column (`M03`). A step with a `fail` above zero always has at least one problem pointing back at it.

### Step

```jsonc
{
  "id": 3,                       // 1-based, unique within this report
  "code": "D04",                 // the check's code in validation.md
  "table": "otters",             // the table the step checked
  "column?": "status",           // the column, dotted for a struct field
  "columns?": ["site", "day"],   // the key's columns, for a composite-key step
  "assertion?": "weight > 0",    // the expression, for an assertion step
  "evaluated": true,
  "units?": 91043,               // test units the step weighed
  "fail?": 2                     // units that failed; pass is units - fail
}
```

Steps are in dictionary order: tables as `tables` declares them, then each table's columns in order, then that column's checks by code.

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

`units` is how many things the step weighed: the table's rows for a row-level data check, and `1` for a check with a single verdict — an aggregate assertion, or any metadata check. `fail` is how many of them failed, and is `0` for a step that passed. Both are absent when `evaluated` is false.

`evaluated` is false when the step could not reach a verdict: the values were not comparable (`D03`, `D06`), the expression could not be run (`D08`, `D09`), or the table's data could not be read, which leaves every step of that table unevaluated. A step that did not evaluate has not passed, and a consumer must not count it as one.

A step carries no values, only counts, so nothing on it is ever withheld for a [restricted](spec.md#display) column.

Nothing is duplicated between the two lists: a step says how much was checked and how much failed, and its problems say what failed and where. A step-by-step report table is `steps` joined to `problems` on `id`, and that join is the consumer's to make, for the same reason `problems` is not also grouped by table or by code.

### Location

A `Location` is a span of the `data-dict.yaml` file — never of the data. Even a `D##` problem locates itself in the dictionary, at the place where the dictionary makes the claim the data broke; the data side is reported as row numbers instead.

```jsonc
{ "start_line": 0, "start_column": 2, "end_line": 0, "end_column": 9 }
```

Lines and columns count from 0, following the LSP convention. Diagnostics rendered for a person show the same positions 1-based.

`location` is the offending node itself, and `context` the nodes enclosing it — for a bad value in a column, the table's name and the column's name. Together they are everything a rendered diagnostic highlights, so a consumer can draw the same annotated excerpt from the document alone. Both are absent for a problem with no place in the file (a pre-flight failure, or a metadata problem about a column the dictionary never mentions).

### Kinds

`kind` names the shape of the finding, and decides which further keys the problem carries. Each kind maps to at most one check code; consult `validation.md` for what the check means.

| `kind` | `code` | Additional keys |
|--------|--------|-----------------|
| `io` | — | |
| `parse` | — | |
| `schema` | — | |
| `parquet` | — | |
| `table_not_found` | — | `available` |
| `spec` | `S01`–`S31` | |
| `type_mismatch` | `M01` | `declared`, `actual` |
| `missing_in_data` | `M02` | |
| `extra_in_data` | `M03` | `actual` |
| `missing_source` | `M04` | |
| `unreadable_source` | `M05` | |
| `nulls_in_required` | `D01` | `count`, `rows` |
| `duplicate_values` | `D02` | `columns`, `count`, `rows` |
| `uniqueness_not_verified` | `D03` | `columns`, `reason` |
| `values_outside_enum` | `D04` | `count`, `rows`, `values`, `redacted` |
| `foreign_key_not_found` | `D05` | `column`, `references`, `count`, `rows`, `values`, `redacted` |
| `referential_integrity_not_verified` | `D06` | `column`, `references`, `reason` |
| `assertion_violated` | `D07` | `assertion`, `count`, `rows`, `samples`, `redacted` |
| `assertion_false` | `D07` | `assertion` |
| `assertion_not_checked` | `D08` | `assertion`, `column`, `reason` |
| `assertion_overflow` | `D09` | `assertion`, `row` |

: {tbl-colwidths="[30,10,60]"}

The four kinds with no code are pre-flight failures: the file couldn't be read (`io`), isn't YAML (`parse`), doesn't match the schema (`schema`), or a Parquet file failed mid-read (`parquet`). `spec` covers every semantic spec check, whose code varies per check.

`values_outside_enum` and `assertion_violated` also arise for a nested column: `column` is then the dotted path to the struct field, and its rows are the rows of the top-level column that holds it.

## Counting and sampling

Three keys describe how many rows broke a check and which ones:

* `count` is the **exact** total number of offending rows. It is never capped.
* `rows` is a **sample** of the offending row numbers, ascending. Row numbers are 1-based and absolute within the table's Parquet file, so they survive row-group boundaries and can be used to seek back into the data.
* `values` (`D04`, `D05`) are the **distinct** offending values, in first-seen order. Because they are deduplicated, `values` is generally shorter than `rows` and its entries do not line up with it.

`samples` (`D07`) is the exception that does line up: one entry per row in `rows`, in the same order, giving that row's relevant columns as an object keyed by column name.

```jsonc
"rows": [12, 91],
"samples": [{"weight": "-1.5", "length": "60"}, {"weight": "0", "length": "74"}]
```

An offending value is always a **string**, in both keys, and never a JSON number or boolean. Each value renders the way the same value would be written as a `range` bound or an `examples` entry in the dictionary itself: a number in decimal at full precision, a date or datetime as ISO 8601, a boolean as `"true"` or `"false"`, a non-finite float as `"NaN"`, `"Infinity"`, or `"-Infinity"`.

A value that is *missing* is the one thing that isn't a string: it is JSON `null`, so it stays distinct from a string column that really holds `"null"`. It can only appear in `samples` — `D04` and `D05` exempt nulls, so `values` never contains one.

`rows`, `values`, and `samples` are each capped, at 1000 entries by default, so that a wholly broken column can't turn a report into megabytes of row numbers. A producer may offer a way to raise or lower that; `count` is never capped, so `count > rows.length` means the sample was truncated. The converse doesn't hold — some checks report a count with no rows at all.

### What each check can say

A check reports what its evidence supports, and the evidence differs. These are properties of the checks themselves, not gaps to be worked around:

* `D01` and `D02` can sometimes be settled from Parquet footer statistics alone, without reading a single value. That path proves how many rows offend but not which, so `count` is present and `rows` is empty.
* `D02` has no `values`: the uniqueness pass keeps a set of keys, not their renderings. Its `rows` are the rows of the **repeat** occurrences — the row that first held a value is not the one reported as a duplicate.
* `D07` on an aggregate assertion (`SUM(x) > 0`) reports `assertion_false`, with no `count` and no `rows`. An aggregate is a single verdict about the whole table, so no row is to blame.
* `D09` reports a single `row`: evaluation stops at the first overflow, since everything after it is computed from a value that already went wrong.
* `D03`, `D06`, and `D08` report that a check *didn't run*, and carry a `reason` naming the barrier rather than any row.

## Restricted columns

A column marked [`display: restricted`](spec.md#display) holds data that must not be surfaced by default. A validation report is user-facing output like any other, and one it is easy to forget: a CI job that archives its reports would otherwise leave restricted values sitting in a build artifact.

So a problem about a restricted column — or about an assertion that reads one — reports `count` and `rows` but no `values`, and sets `"redacted": true`. The rows are still there, so a consumer can still find the offending records in the data it is already entitled to read; only the values are withheld.

Redaction in `samples` is per column: a restricted column's key is left out of each entry, while its unrestricted neighbours keep their values. If the assertion reads nothing but restricted columns there is nothing left to say, so `samples` is omitted rather than given as a list of empty objects.

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
     "evaluated": true, "units": 1, "fail": 0},
    {"id": 2, "code": "D01", "table": "otters", "column": "id",
     "evaluated": true, "units": 91043, "fail": 0},
    {"id": 3, "code": "D02", "table": "otters", "column": "id", "columns": ["id"],
     "evaluated": true, "units": 91043, "fail": 3},
    {"id": 4, "code": "D04", "table": "otters", "column": "carer_email",
     "evaluated": true, "units": 91043, "fail": 2},
    {"id": 5, "code": "D07", "table": "otters", "column": "weight", "assertion": "weight > 0",
     "evaluated": true, "units": 91043, "fail": 2},
    {"id": 6, "code": "M04", "table": "sightings",
     "evaluated": true, "units": 1, "fail": 1},
    // the source couldn't be read, so no step of `sightings` reached a verdict
    {"id": 7, "code": "D01", "table": "sightings", "column": "otter_id", "evaluated": false}
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
      "column": "id",
      "columns": ["id"],
      "count": 3,
      "rows": [118, 4092, 91043],
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
      "column": "carer_email",
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
      "column": "weight",
      "assertion": "weight > 0",
      "count": 2,
      "rows": [12, 91],
      "samples": [{"weight": "-1.5"}, {"weight": "0"}],
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
jq '.steps | map({step: .id, code, table, column, units,
                  pass: (if .evaluated then .units - .fail else null end), fail})' report.json
```
