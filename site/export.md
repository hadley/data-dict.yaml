# Export

`data-dict` can render a `data-dict.yaml` file to JSON, resolving everything the validator already computes internally — parsed types, constraint flags, joins, assertions — so downstream tools don't need to re-implement any of the spec's parsing or inference rules.

## Two levels

Export has two levels, mirroring [validation](validation.md):

* **`export-spec`** renders the dictionary itself: every table, column, relationship, and glossary entry, fully resolved. It reads only the `data-dict.yaml` file, never the data. Internally this runs the same `validate-spec` pass and serializes the resulting model, so `export-spec` fails with the same `S##` diagnostics as `validate-spec` if the file is invalid.

* **`export-data`** does everything `export-spec` does, and also profiles each table's source data: distinct/missing counts, sample values, and a histogram (numeric and temporal columns) or top values (string, boolean, and enum columns) per column — the same profiling `describe` performs on a single Parquet file, run here across every table via each one's `source`. This is the only level that reads the data, so it can be expensive. It does *not* validate the data against the dictionary — that's `validate-meta`/`validate-data`'s job: a source that's missing (M04) or unreadable (M05) is reported as a warning and that table's profiles are omitted, and a declared column the data doesn't have simply gets no `profile`, so a partially-sourced dictionary still exports everything it can.

Both levels emit the same JSON document shape; `export-spec` just never populates the `profile` fields.

## Output shape

A key with nothing to say is **omitted** rather than serialized as `null` or `[]`: keys marked `?` below may be absent, meaning the value wasn't declared (or, for a profile statistic, couldn't be established). Zeroes and falses are real data and always appear. Consumers should read absent and null interchangeably — `jq`, JavaScript property access, and optional-aware decoders already do.

Prose fields — every `description` and `details`, and each glossary `definition` — are written as Markdown in the dictionary and arrive here rendered to HTML (any raw HTML in the source is escaped rather than passed through), so consumers can place them straight into a page without a Markdown implementation of their own.

### Top level

```jsonc
{
  "$version": "0.1.0",           // version of the export document format itself
  "name?": "string",
  "label?": "string",
  "description?": "string",
  "details?": "string",
  "origin?": "string",
  "learn_more?": "string",
  "version?": { "number": "1.2.3" } | { "date": "2024-01-31" } | { "hash": "..." },
  "tables": [ Table ],
  "relationships?": [ Relationship ],
  "glossary?": [ { "term": "string", "definition": "string" } ]
}
```

### Table

```jsonc
{
  "name": "string",
  "label?": "string",
  "description?": "string",
  "details?": "string",
  "origin?": "string",
  "source?": { "parquet": "path/relative/to/dictionary" },
  "rows?": 123,                  // the source data's row count; export-data only,
                                 // absent when the table's source wasn't profiled
  "columns": [ Column ],
  "constraints?": [ Assertion ]  // table-level `assert` entries
}
```

### Column

A `Column` is recursive: `fields` holds child `Column`s for `struct` and `list(struct)`; a field uses the same shape, minus the keys the spec doesn't allow on fields (`label`, `display`, `constraints`). A column or field with no declared `type` makes no claims and is omitted from the export entirely, including from `COLUMNS(...)` expansions:

```jsonc
{
  "name": "string",
  "label?": "string",
  "description?": "string",
  "details?": "string",
  "display?": "restricted",
  "type": "string",              // canonical type, e.g. "list(number(quantity))"
  "units?": "string",
  "time_zone?": "string",
  "constraints?": ["primary_key" | "foreign_key" | "unique" | "required", ...],
  // constraints as declared PLUS those implied by other constraints
  // (e.g. primary_key implies both unique and required)
  "references?": { "table": "string", "column": "string" },
  // present when this column is a `foreign_key`: the primary-key column it points at
  "referenced_by?": [ { "table": "string", "column": "string" } ],
  // present when this column is a `primary_key` that foreign-key columns
  // elsewhere reference: each of those columns
  "values?": [Scalar],            // enum
  "range?": { "min": Scalar, "max": Scalar },
  "examples?": [Scalar],
  "fields?": [ Column ],
  "assertions?": [ Assertion ],   // column-level `assert` entries
  "profile?": Profile             // export-data only; never present under export-spec
}
```

Both `references` and `referenced_by` are derived from `relationships`, not read directly off the column — they're absent for a column that isn't part of any relationship, even if it's marked `foreign_key`/`primary_key` in isolation (which `validate-spec`'s S01 already treats as an error).

### Assertion

```jsonc
{
  "expression": "string",         // original `assert` text
  "description?": "string",
  "columns": ["string", ...]      // every column (and struct field, dotted) the expression references
}
```

### Relationship

A `Relationship` is normalized so cardinality is always read left-to-right as "many-to-one" (a declared `one-to-many` has each pair's `left`/`right` swapped so `left` is always the "many" side and `right` the "one" side; `one-to-one` is unaffected):

```jsonc
{
  "description?": "string",
  "cardinality": "one-to-one" | "many-to-one",
  "declared_cardinality": "one-to-one" | "one-to-many" | "many-to-one",
  // the cardinality as written — the orientation the `join` text documents
  "pairs": [ { "left": ColumnRef, "right": ColumnRef } ],
  // ColumnRef = { "table": "string", "column": "string" }
  "join": "string",               // original join text, unnormalized
  "aliases?": [ { "name": "string", "table": "string" } ],
  "conflicts?": ["string", ...]
}
```

`pairs` records which column matches which, one pair per join conjunct. Pair tables are real table names — an alias in the `join` is resolved through `aliases`, which is preserved so the original `join` text can still be read. A range join like `a.date >= b.start AND a.date <= b.end` yields two pairs: `a.date` ↔ `b.start` and `a.date` ↔ `b.end`.

### Profile

A `Profile` (export-data only) accompanies one column, carrying the same statistics as `describe`. Its shape follows the column's type, so a key that could never apply to that type doesn't appear at all. (Strictly, the shape follows the *data's* type — the two agree whenever the data conforms to the dictionary, which `validate-meta` can confirm.)

Numeric and temporal columns (`number`, `number(...)`, `date`, `datetime`) summarize on a scale:

```jsonc
{
  "distinct?": { "count": 123, "approximate": false },
  // absent for a continuous (float) column, where per-value equality is
  // misleading — its shape is the histogram
  "missing?": 4,
  "range?": { "min": Scalar, "max": Scalar },   // the observed extremes
  "sample_values?": [Scalar, ...],
  "histogram?": {
    "bins": [
      { "min": 0, "max": 10, "count": 5, "closed": "right" | "both" },
      ...
    ],
    // float values with no place on the number line, counted apart from the
    // bins; each appears only when nonzero
    "nan_count?": 1,
    "negative_infinity_count?": 1,
    "positive_infinity_count?": 1
  }
}
```

String, boolean, and enum columns summarize by value:

```jsonc
{
  "distinct?": { "count": 123, "approximate": false },
  "missing?": 4,
  "sample_values?": [Scalar, ...],
  "common_values?": {
    "approximate": false,
    "values": [ { "value": Scalar, "count": 42 }, ... ]
  }
}
```

`sample_values` are up to 20 representative values, spread along the column's sorted distinct values rather than drawn from its start, and reported exactly — never rounded, since how much precision a value carries is part of what it tells you. They are omitted for a column that declares its `values`: that declaration is exhaustive, so it already gives the reader every value the column can hold, in full rather than as a sample.

A `list` column reports only its containers — `{ "missing": 4 }`, the null-list count — never its elements.

Each histogram bin's `closed` says which of its boundary values it includes: every bin is `"right"` (`(min, max]`) except the first, which is `"both"` (`[min, max]`) so the column minimum has a home; bins are otherwise contiguous.

Nested columns profile as far as the data allows:

* A `struct` column carries no `profile` — its fields carry their own instead. A field reached through a list layer (`list(struct)`) is profiled per element, so its counts are over elements rather than rows.
* A list-typed *field* inside a struct carries no `profile` at all (its container nulls aren't countable below the top level).
* A column whose Parquet type can't be summarised (uuid, decimal, json, …) gets the list shape — `missing` alone, when the file's footer supplies it — or no `profile` at all.

### Scalar

A `Scalar` is a literal JSON value: a number, string, boolean, or `null`, following the same rendering `range`/`examples`/`values` already use elsewhere. An infinite range bound (`.inf`), which JSON can't spell, renders as `null` — that end of the range is open.
