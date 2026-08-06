# Export

`data-dict` can render a `data-dict.yaml` file to JSON, resolving everything the validator already computes internally — parsed types, constraint flags, joins, assertions — so downstream tools don't need to re-implement any of the spec's parsing or inference rules.

## Two levels

Export has two levels, mirroring [validation](validation.md):

* **`export-spec`** renders the dictionary itself: every table, column, relationship, and glossary entry, fully resolved. It reads only the `data-dict.yaml` file, never the data. Internally this runs the same `validate-spec` pass and serializes the resulting model, so `export-spec` fails with the same `S##` diagnostics as `validate-spec` if the file is invalid.

* **`export-data`** does everything `export-spec` does, and also profiles each table's source data: distinct/missing counts, sample values, and a histogram (numeric and temporal columns) or top values (string, boolean, and enum columns) per column — the same profiling `describe` performs on a single Parquet file, run here across every table via each one's `source`. This is the only level that reads the data, so it can be expensive. It runs the full metadata- and data-level checks before profiling and fails with `M##`/`D##` diagnostics the same way `validate-meta`/`validate-data` do — but unlike those commands, a source that's missing (M04) or unreadable (M05) for one table is reported as a warning and that table's `profile` fields are simply left null, rather than stopping the whole export (so a partially-sourced dictionary still exports everything it can).

Both levels emit the same JSON document shape; `export-spec` just never populates the `profile` fields.

## Output shape

Top level:

```jsonc
{
  "name": "string or null",
  "label": "string or null",
  "description": "string or null",
  "details": "string or null",
  "origin": "string or null",
  "learn_more": "string or null",
  "version": { "number": "1.2.3" } | { "date": "2024-01-31" } | { "hash": "..." } | null,
  "tables": [ Table ],
  "relationships": [ Relationship ],
  "glossary": [ { "term": "string", "definition": "string" } ]
}
```

`Table`:

```jsonc
{
  "name": "string",
  "label": "string or null",
  "description": "string or null",
  "details": "string or null",
  "origin": "string or null",
  "source": { "parquet": "path/relative/to/dictionary" } | null,
  "columns": [ Column ],
  "constraints": [ Assertion ]   // table-level `assert` entries
}
```

`Column` (recursive: `fields` holds child `Column`s for `struct` and `list(struct)`; a field uses the same shape, with the keys the spec doesn't allow on fields — `label`, `display`, `constraints` — empty or null):

```jsonc
{
  "name": "string",
  "label": "string or null",
  "description": "string or null",
  "details": "string or null",
  "display": "restricted" | null,
  "type": "string or null",      // canonical type, e.g. "list(number(quantity))";
                                 // null when the column declares no `type`
  "units": "string or null",
  "time_zone": "string or null",
  "constraints": ["primary_key" | "foreign_key" | "unique" | "required", ...],
  // constraints as declared PLUS those implied by other constraints
  // (e.g. primary_key implies both unique and required)
  "references": { "table": "string", "column": "string" } | null,
  // present when this column is a `foreign_key`: the primary-key column it points at
  "referenced_by": [ { "table": "string", "column": "string" } ],
  // present when this column is a `primary_key`: every foreign-key column elsewhere
  // that references it (empty array if none)
  "values": [Scalar] | null,      // enum
  "range": { "min": Scalar, "max": Scalar } | null,
  "examples": [Scalar] | null,
  "fields": [ Column ] | null,
  "assertions": [ Assertion ],    // column-level `assert` entries
  "profile": Profile | null       // export-data only; null under export-spec
}
```

Both `references` and `referenced_by` are derived from `relationships`, not read directly off the column — they're `null`/`[]` for a column that isn't part of any relationship, even if it's marked `foreign_key`/`primary_key` in isolation (which `validate-spec`'s S01 already treats as an error).

`Assertion`:

```jsonc
{
  "expression": "string",         // original `assert` text
  "description": "string or null",
  "columns": ["string", ...]      // every column (and struct field, dotted) the expression references
}
```

`Relationship`, normalized so cardinality is always read left-to-right as "many-to-one" (a declared `one-to-many` has its `left`/`right` swapped so `left` is always the "many" side and `right` the "one" side; `one-to-one` is unaffected):

```jsonc
{
  "description": "string or null",
  "cardinality": "one-to-one" | "many-to-one",
  "left": { "table": "string", "columns": ["string", ...] },
  "right": { "table": "string", "columns": ["string", ...] },
  "join": "string",               // original join text, unnormalized
  "aliases": [ { "name": "string", "table": "string" } ],
  "conflicts": ["string", ...]
}
```

`left.table` and `right.table` are real table names — an alias in the `join` is resolved through `aliases`, which is preserved so the original `join` text can still be read.

`Profile` (export-data only), one per column, shaped like `describe`'s per-column output:

```jsonc
{
  "distinct": { "count": 123, "approximate": false } | null,
  // null for a continuous (float) column, where per-value equality is
  // misleading — its shape is the histogram
  "missing": 4,                   // null when the count couldn't be established
  "sample_values": [Scalar, ...],
  "histogram": {
    "bins": [
      { "min": 0, "max": 10, "count": 5, "closed": "right" | "both" },
      ...
    ]
  } | null,
  "common_values": {
    "approximate": false,
    "values": [ { "value": Scalar, "count": 42 }, ... ]
  } | null
}
```

`histogram` is populated for numeric and temporal columns, `common_values` for string, boolean, and enum columns; the other is `null` (an untyped column follows whatever its data turns out to be). Each histogram bin's `closed` says which of its boundary values it includes: every bin is `"right"` (`(min, max]`) except the first, which is `"both"` (`[min, max]`) so the column minimum has a home; bins are otherwise contiguous.

Nested and untyped columns profile as far as the data allows:

* A `struct` column's `profile` is `null` — its fields carry their own profiles instead. A field reached through a list layer (`list(struct)`) is profiled per element, so its counts are over elements rather than rows.
* A `list`-typed column's `profile` describes the list column itself, not its elements: `missing` counts null containers, and the per-value keys (`distinct`, `sample_values`, `histogram`, `common_values`) stay null/empty. A list-typed *field* inside a struct carries no `profile` at all.
* A column whose Parquet type can't be summarised (uuid, decimal, json, …) gets a profile with only `missing` populated, when the file's footer supplies it.

`Scalar` is a literal JSON value: a number, string, boolean, or `null`, following the same rendering `range`/`examples`/`values` already use elsewhere. An infinite range bound (`.inf`), which JSON can't spell, renders as `null` — that end of the range is open.

## CLI

```
data-dict export-spec [PATH] [--pretty]
data-dict export-data [PATH] [--pretty]
```

`PATH` is a file or a directory containing `data-dict.yaml` (defaults to `.`), matching `validate-spec`. Output is JSON on stdout; `--pretty` pretty-prints it (default is compact, one document per line, so it composes with tools like `jq`). Diagnostics (the `S##`/`M##`/`D##` problems) are printed to stderr in the same format `validate-*` uses, and a non-zero exit code is returned exactly when no document could be produced — the level's validation failed. A missing or unreadable source is a warning, so a partially-profiled export still exits zero.
