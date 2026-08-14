# Validation

## Three levels of validation

Validation happens at three levels, each a strict superset of the one before it:

* Validating the **spec** checks that the dictionary file itself conforms to the data-dict spec — that it is well-formed and internally consistent, with valid types, foreign keys that have matching relationships, joins that parse, and so on. These checks have an unambiguous right answer, so most are errors. This level looks only at the `data-dict.yaml` file, never at the data. This is performed by `data-dict validate-spec`.

* Validating the **metadata** checks that the data's column names and types match the dictionary. It reads only the data's metadata (for example, a Parquet file's footer), not its values, so it stays cheap. This is performed by `data-dict validate-meta`.

* Validating the **data** checks that the data's values match the dictionary — that required columns have no nulls, and so on. This is the only level that reads the data itself, so it can be expensive, depending on the data source. This is performed by `data-dict validate-data`.

The last two levels compare the dictionary against the data (or equivalently, the data against the dictionary). When they disagree, we can't tell which side needs to change. If you're creating the dictionary as you learn about the data, then you might need to change the dictionary. If you're using the dictionary to validate a dataset, there might be an upstream issue that you need to resolve.

The metadata and data levels locate each table's data through its [`source`](spec.md#source): they read the file the table's `source.parquet` points at, resolved relative to the dictionary file. They validate **every** table in the dictionary, each against its own source, so a single run checks the whole dictionary. A problem in one table (an unreadable source, a column mismatch) is reported against that table and does not stop the others from being checked.

Each level implies the ones before it: validating the metadata validates the spec first, and validating the data validates both the spec and the metadata first. Validating the spec and metadata are cheap, so they can be run continually while you edit the `data-dict.yaml`; validating the data adds a full scan and get more expensive as the size of the data increases.

Each check has a code prefixed by its level: spec checks are `S01`, `S02`, …; metadata checks `M01`, …; data checks `D01`, …. Every code a validator reports is in this file: the codes are part of the interface, so a program can dispatch on them and they don't change with the tool's internals. Severity is independent of level — any level can raise errors or warnings.

## Errors vs warnings

A validator reports two severities of problem: **errors** and **warnings**. The distinction is about urgency, not importance.

* An **error** means the dictionary is invalid or there's a critical mismatch between the data and dictionary. Errors will cause a production pipeline to fail, and you must fix them immediately.

* A **warning** means the dictionary is usable but the data and dictionary may have drifted apart. Warnings will not cause a production pipeline to fail, but if you're actively working on the project you should make sure to fix them.

## Structural spec checks

Validating the spec starts by checking the document's shape: that every key the spec requires is present, that no key it doesn't define appears, and that each value has an allowed type. These checks are about the document as a data structure, so they say nothing about what it means — a `foreign_key` that points nowhere is a well-shaped document, and is caught by a `S01`–`S31` check below.

Their codes are the `S60` block, reserved for structural checks:

| Code | Name | Sev | Description |
|------|------|-----|-------------|
| S60 | Missing required key | E | A mapping is missing a key the spec requires, such as a table without `columns`. |
| S61 | Wrong value type | E | A value's type is not one the spec allows there, such as a `description` given a list instead of a string. |
| S62 | Value not allowed | E | A value is not among the fixed set the spec allows there, such as a `display` other than `default`, `hidden`, or `restricted`. |
| S63 | Empty mapping | E | A mapping the spec requires at least one entry in is empty, such as a table with no columns. |
| S64 | Unknown key | E | A mapping contains a key the spec doesn't define. A misspelled key reports this rather than the `S60` for the key it was meant to be, and the two are reported together when both apply. |
| S65 | Duplicate key | E | The same key appears twice in one mapping. |
| S69 | Structural violation | E | The document breaks a structural rule none of the above covers. A validator reports this only as a last resort; `expected` and `message` say what the rule was. |

: {tbl-colwidths="[7,23,5,65]"}

The document is checked whole, so every structural problem in it is reported at once, each at its own location. A structural error stops validation before the checks below run: they read the document as the typed model the spec describes, which a misshapen document can't be read as.

## Spec-validation checks

These are the semantic checks: the document is shaped correctly, and they ask whether what it says is consistent.

| Code | Name | Sev | Description |
|------|------|-----|-------------|
| S01 | Unresolved foreign key | E | A `foreign_key` column has no `relationships` entry pointing it at a `primary_key` column. |
| S02 | Unknown table | E | A relationship references a table that is not defined in `tables`, either directly in its `join` or as the target of an `aliases` entry. |
| S03 | Unknown column | E | A relationship references a column that does not exist on its table. |
| S04 | Invalid join | E | A `join` expression fails to parse, or references more than two tables. |
| S05 | Unresolved conflict column | E | A name in `conflicts` is not a column on both sides of the join. |
| S06 | Inconsistent cardinality | E | The declared cardinality is inconsistent with the constraints on the joined columns (e.g. `one-to-many` whose "one" side is not `primary_key` or `unique`). |
| S07 | Wrong representation key | E | A column's data representation key is absent or wrong for its type (`enum` → `values`; `number(ordinal)`, `number(quantity)`, `date`, `datetime` → `range`; otherwise → `examples`). A numeric or temporal column may also carry the other of `range`/`examples` alongside the one it requires; `string` takes `examples` alone. A `boolean` column must carry none of `values`, `range`, or `examples`. For `list(element_type)` the element type (the innermost, for nested lists) picks the key; a column whose innermost element type is `struct` instead requires `fields` and takes no representation key, and `fields` is invalid on any other type. |
| S08 | Units without quantity | E | A column has `units` but its type is not `number(quantity)` — or a list whose innermost element type is. |
| S09 | Missing `$learn_more` | W | The document omits the recommended `$learn_more` key. |
| S10 | Duplicate name | E | Two tables within a dictionary share a `name`, or two columns/definitions within the same table share a `name`. |
| S11 | Empty name | E | A table, column, or definition `name` is empty. |
| S12 | Wrong value type | E | A value in `range` or `examples` does not match the column's `type` — a number type wants numbers; `string` wants strings, so a value that reads as a number or a boolean counts only if quoted; `date` wants an ISO 8601 date (e.g. `2024-01-31`); `datetime` wants an ISO 8601 datetime, with an offset (e.g. `2024-01-31T09:30:00Z`) unless the column has a `time_zone`, in which case it's zoneless (e.g. `2024-01-31T09:30:00`). A `range` bound may instead be `-.inf` (minimum) or `.inf` (maximum) to leave that end open, on any range type. `.nan` is never valid in either key: a NaN has no place on the number line, so it can neither bound a range nor stand for an observed value — and, having no JSON spelling, it would [export as `null`](export.md#scalar) and read as an open bound. For a `list(element_type)` column the values must match the element type (the innermost, for nested lists). |
| S13 | Descending range | E | A `range`'s minimum is greater than its maximum. An open bound counts as ordered only in its own place — `-.inf` as the minimum and `.inf` as the maximum; `.inf` as a minimum or `-.inf` as a maximum runs backwards. |
| S14 | Time zone without datetime | E | A column has `time_zone` but its type is not `datetime` — or a list whose innermost element type is. |
| S15 | Malformed time zone | E | A `time_zone` is not `naive`, `UTC`, or an IANA `Area/Location` name with a known area. The shape is checked, not the full tz database, so the accepted set doesn't go stale as zones are added or renamed. |
| S16 | Misplaced single-table description | W | A dictionary with exactly one table carries `label`, `description`, or `details` on that table; for a single-table dictionary these belong at the top level. |
| S17 | Malformed version | E | The top-level `version` does not give exactly one of `number`, `date`, or `hash`; its `number` is not three dot-separated numeric components (`MAJOR.MINOR.PATCH`) with an optional pre-release/build suffix; or its `date` is not a valid ISO 8601 date (`YYYY-MM-DD`). |
| S18 | Missing `$version` | E | The document omits the required top-level `$version` key. |
| S19 | Malformed expression | E | An expression fails to parse (a syntax error in the [expression language](expressions.md)). |
| S20 | Unknown name in an expression | E | An expression uses an unknown column or definition, a `COLUMNS([...])` list uses an unknown column, or a field access uses an unknown field. |
| S21 | Ill-typed expression | E | An expression is syntactically valid but semantically wrong: an operator or function applied to the wrong operand type (including a column a `COLUMNS(...)` selects, and an argument outside a signature's [type class](expressions.md#type-classes), such as `SUM` of a string), a wrong function arity, a non-boolean top-level `assert` expression, more than one `COLUMNS(...)`, a malformed `SIMILAR TO` / `COLUMNS('...')` regex, a field access on anything but a `struct` (including through a `list`), or a bare `struct` or `list` column used where a value is needed (anywhere but `IS [NOT] NULL` and `COUNT`, which ask only whether a value is null). |
| S22 | Empty column selection | W | A `COLUMNS('<regex>')` in an expression matches no columns on the table. |
| S23 | Untyped column in an expression | E | An expression uses a column listed by name only (or a `struct` field with no declared `type`) somewhere its type matters, so the expression can't be checked. Declaring the column's `type` fixes it. Operands whose type is never consulted (`IS NULL`, `IS NOT NULL`, and `COUNT`, which all ask only whether a value is null) are exempt. |
| S24 | Invalid enum values | E | An `enum`'s `values` are not a non-empty set of strings: they are empty (`[]` or `{}`), so nothing is permitted, or a value is not a string — a number, a boolean, null, or a nested list/map. A category that reads as a number or a boolean has to be quoted to be a string (`'1'`, `'-9'`, `'true'`). Both forms are checked: the list items, and the keys of the map form. |
| S25 | Unaliased self-join | E | Both sides of a `join` denote the same rows: the same name appears on both sides, or both sides resolve to the same table without each being a distinct alias. A self-join must name each side with its own `aliases` entry. |
| S26 | Alias shadows table | E | An `aliases` key has the same name as a table in `tables`, so a name in the `join` could be read either way. |
| S27 | Unused alias | W | An `aliases` entry is declared but never referenced by the relationship's `join`. |
| S28 | Invalid type | E | A column's `type` is not a recognised type string. Valid types are the fixed scalars (`string`, `number`, `number(id)`, `number(ordinal)`, `number(quantity)`, `boolean`, `date`, `datetime`), `enum`, `struct`, and `list(element_type)` where the element type is any of the above — including another `list(...)`, nested to any depth. |
| S29 | Invalid constraint on list or struct | E | A `primary_key`, `foreign_key`, or `unique` constraint appears on a `list` or `struct` column. (There is no such check for fields: a field can't carry `constraints` at all, which the schema enforces structurally.) |
| S30 | Nested aggregate | E | An aggregate function's argument contains another aggregate call, as in `AVG(MIN(x))`: an aggregate folds one value per row, and another aggregate gives it a single value. This is the only [shape](expressions.md#shapes) an expression can get wrong — every other combination is legal, including mixing a row-level subexpression with an aggregate one (`value <= 2 * MIN(value)`). |
| S31 | Unresolved todo | W | A `todo` key remains in the dictionary. Each one is reported at its location. Delete the key once the work it records is done. |
| S32 | Unsupported spec version | E | The document's `$version` names a spec version this validator can't accept: it is not a valid version number, it predates the first spec version (`0.1.0`), or it is newer than the version the validator supports (currently `0.1.0`). A newer version means the tool is older than the document — upgrade `data-dict`. Versions between the first and the supported one are accepted. |
| S33 | Definition shadows column | E | A definition's `name` matches a column name in the same table; definitions and columns share a namespace, so a reference to that name would be ambiguous. |
| S34 | Circular definition reference | E | Following the definition references from a definition leads back to the definition itself. |

: {tbl-colwidths="[7,23,5,65]"}

(That each of an `enum`'s `values` is a scalar, and each label in the map form a string, is constrained structurally by the schema rather than by an `S` check; S24 covers what the schema can't reach, including the keys of the map form. The schema deliberately keeps admitting a number or boolean among the list items so that S24 reports an unquoted category itself, rather than the reader meeting a structural "expected array, got object" from a failed branch match. The `version` map's allowed keys and their value types are likewise structural, with S17 covering the rest.)

## Metadata-validation checks

| Code | Name | Sev | Description |
|------|------|-----|-------------|
| M01 | Type mismatch | E | A column's declared type is incompatible with the data. A `list(element_type)` column requires list-shaped data whose element type is compatible; a `struct` column requires struct-shaped data. |
| M02 | Missing column | E | A column the dictionary describes is absent from the data. This applies even to columns listed by name only — listing a column that doesn't exist is an error. |
| M03 | Undocumented column | W | A column present in the data that the dictionary does not describe. This is a warning, not an error: if a production pipeline adds a column, validation should not fail, but you should document it (or at least list it by name) next time you touch the dictionary. |
| M04 | Missing source | E | A table validated against data does not declare a `source`. `source` is optional at the spec level but required here, so a validated dictionary always records where its data comes from. |
| M05 | Unreadable source | E | A table declares a `source`, but its data can't be read — the `source.parquet` file is absent, or present but not a readable Parquet file. The path is resolved relative to the dictionary file. |

: {tbl-colwidths="[7,23,5,65]"}

M01–M03 descend into `struct` fields, recursively and through `list(struct)`: each declared field is checked like a column against the corresponding child of the data's struct type — its type must be compatible (M01), a declared field must exist in the data (M02, error), and a child the dictionary doesn't describe is reported (M03, warning).

### Nested Parquet types {#nested-parquet-types}

How Parquet's nested shapes read as data-dict types, for the M01 comparison:

* A `LIST`-annotated group reads as `list(element_type)`. So does a legacy repeated field (the pre-`LIST` two-level encoding): a `repeated int32 xs` outside a `LIST` wrapper means the same thing, a list of values per row.
* A plain group reads as `struct`.
* A `MAP`-annotated group reads as none of the data-dict types: its keys are data, not schema, so no `fields` can describe it. A documented map column always reports M01; an undocumented one is reported by M03 like any other column.

## Data-validation checks

| Code | Name | Sev | Description |
|------|------|-----|-------------|
| D01 | Nulls in a required column | E | A `required` or `primary_key` column contains nulls. On a `list` or `struct` column, `required` is about the container itself: a null list counts, an empty one does not. A [NaN or an infinity](floating-point.md#non-finite) is a value rather than a missing one, so a float column holding them satisfies `required`; assert `IS_FINITE(x)` if they should be excluded too. |
| D02 | Duplicate values | E | A `unique` column contains duplicate values, or the combination of all `primary_key` columns does not uniquely identify every row. Only [comparable types](#comparable-types) are checked. Null/missing values are never counted as duplicates; for a composite primary key, a row with a null in any key column is not compared. |
| D03 | Uniqueness not verified | W | A `unique` column or `primary_key` uses a type whose values can't be reliably compared, so its uniqueness was not checked. |
| D04 | Value outside enum | E | An `enum` column contains a (non-null) value that is not one of its declared `values`. Applies inside nested types too: the elements of a `list(enum)` column, and every `enum` field reachable through `struct` fields (recursively, including through `list(struct)`), are checked against their declared `values`. |
| D05 | Foreign key not found | E | A `foreign_key` column contains a (non-null) value that does not appear in the `primary_key` column it references. Only [comparable types](#comparable-types) are checked; null/missing values are exempt (a null foreign key references nothing). Only single-column foreign keys are checked. |
| D06 | Referential integrity not verified | W | A `foreign_key` column, or the `primary_key` it references, uses a type whose values can't be reliably compared, so the reference was not checked. |
| D07 | Assertion violated | E | An `assert` [expression](expressions.md) is false. A row-level assertion reports how many rows it is false for and identifies the first few; an aggregate assertion is a single verdict about the table, so it reports only that it is false. A row passes when the expression is true **or** null, so an assertion is never also a null check (see [evaluation](expression-execution.md#what-counts-as-a-violation)). |
| D08 | Assertion not checked | E | An `assert` expression could not be evaluated against the data: it references a column that can't be read as the type the dictionary declares for it, or a `LIKE` / `SIMILAR TO` pattern it reads *from* the data is not a valid regular expression. A literal pattern is rejected at the spec level (S21), so only a computed one can get this far. An unevaluated rule has not been satisfied, so this is an error rather than a silent pass: the dictionary states a rule the data cannot be held to, and one of the two has to change. |
| D09 | Assertion overflowed | E | Integer arithmetic in an `assert` expression left the 64-bit range, so the expression no longer computes what it says. |

: {tbl-colwidths="[7,23,5,65]"}

Dividing by zero is not among these: `7 / 0` is `INF` and `0 / 0` is a NaN, [as the language specifies](floating-point.md#non-finite), and a comparison against a NaN is `false`, so a row whose arithmetic goes non-finite is reported as an ordinary D07 violation.

### Comparable types {#comparable-types}

Lists and structs never reach the comparison checks at all: `unique`, `primary_key`, and `foreign_key` are invalid on them at the spec level (S29), so D02, D03, D05, and D06 only ever see scalar columns.

The uniqueness check (D02) compares values directly, so it only runs on types whose equality is unambiguous. Which types those are depends on the data source, since each source stores values differently. Today the only source is Parquet.

For **Parquet**:

* Numbers, booleans, strings, enums, dates, and datetimes are compared by value. Decimals are compared by numeric value, regardless of how they are encoded. Floating-point values — including 16-bit floats — are compared by [identity rather than by `=`](floating-point.md#non-finite): `-0.0` and `+0.0` are one value, and all NaNs are one value. That is deliberately not the language's `=`, under which no NaN equals any NaN — a uniqueness check asks whether a value has been seen before, which is a question about identity, and answering it with `=` would let a column of a million NaNs pass as a million distinct keys. Legacy `INT96` timestamps are compared as datetimes, by the instant they denote.

* JSON and BSON, whose byte representation does not determine equality (two documents can differ only in whitespace or key order and still be equal), are **not** compared. Neither is any Parquet logical type the validator does not recognize — including future types such as `VARIANT` or `GEOMETRY`.

For a non-comparable column, running the check anyway could silently miss duplicates and pass a dataset that should fail, so the check is skipped with a D03 warning instead. A composite primary key is skipped whole if any of its columns is non-comparable.

The foreign-key check (D05) is governed by the same comparability rule, identity included, so a NaN in a child column matches a NaN in the parent: the foreign-key column and the primary-key column it references are compared by the same normalized value form, so both must be comparable. The two columns need not share a physical representation: values are compared as values, so a key stored as `INT64` can be referenced by an `INT32` column, and a byte-encoded decimal by an int-encoded one. When the two columns have no common comparable form at all (say, a string referencing a number), no value can match, and every non-null child value is reported. If either column uses a non-comparable type, the reference could silently mismatch, so the check is skipped with a D06 warning instead.

### Assertions {#assertions}

An `assert` expression is checked for form at the spec level (S19–S23, S30) and evaluated against the data here. Only an expression that passed every spec check is evaluated, so D07–D09 never report a problem with the expression itself — only with the data it describes.

`NOW()` is bound once for the whole `validate-data` run, so every assertion in the run agrees on the current time. [Executing expressions](expression-execution.md) is the reference for what evaluation means.

D08 and D09 are the two ways an assertion can fail to reach a verdict: its columns can't be read as their declared types or a pattern it reads from the data won't compile, and [its integer arithmetic has no result](expression-execution.md#no-result). Both are errors, and each replaces the D07 that assertion would otherwise report. A rule that could not be computed has neither held nor been broken, and neither is allowed to read as a pass.

### Enum membership {#enum-membership}

An `enum` column's underlying data must be string-like: a Parquet string column, or a true Parquet enum. Any other underlying type is a type mismatch (M01). Its declared `values` are strings, and membership (D04) is plain string equality.

## Reporting

A validation warning or error can be reported in two ways: as a diagnostic rendered for a person, and as a record in a machine-readable [report document](report.md) for a program. Both are views of the same findings, so a pipeline can act on the report and a person can read the diagnostics without the two disagreeing.

The CLI writes the report document with `--json`, and the same report as a self-contained HTML page with `--html`. The page is a rendering of that document and adds nothing to it: what a report withholds, the page withholds.

They differ only in how much they list. A finding always counts the offending rows exactly, but names only the first so many: a report lists enough of them to filter the offending records back out of the data, a diagnostic only enough to see the shape of the problem.

Some checks report the offending values themselves. Those values are withheld for a column marked [`display: restricted`](spec.md#display): the count and the row numbers are still reported, so the records stay findable by anyone already entitled to read them, but the validator never reports the values.
