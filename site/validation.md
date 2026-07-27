# Validation

## Three levels of validation

Validation happens at three levels, each a strict superset of the one before it:

* Validating the **spec** checks that the dictionary file itself conforms to the data-dict spec — that it is well-formed and internally consistent, with valid types, foreign keys that have matching relationships, joins that parse, and so on. These checks have an unambiguous right answer, so most are errors. This level looks only at the `data-dict.yaml` file, never at the data. This is performed by `data-dict validate-spec`.

* Validating the **metadata** checks that the data's column names and types match the dictionary. It reads only the data's metadata (for example, a Parquet file's footer), not its values, so it stays cheap. This is performed by `data-dict validate-meta`.

* Validating the **data** checks that the data's values match the dictionary — that required columns have no nulls, and so on. This is the only level that reads the data itself, so it can be expensive, depending on the data source. This is performed by `data-dict validate-data`.

The last two levels compare the dictionary against the data (or equivalently, the data against the dictionary). When they disagree, we can't tell which side needs to change. If you're creating the dictionary as you learn about the data, then you might need to change the dictionary. If you're using the dictionary to validate a dataset, there might be an upstream issue that you need to resolve.

The metadata and data levels locate each table's data through its [`source`](spec.md#source): they read the file the table's `source.parquet` points at, resolved relative to the dictionary file. They validate **every** table in the dictionary, each against its own source, so a single run checks the whole dictionary. A problem in one table (an unreadable source, a column mismatch) is reported against that table and does not stop the others from being checked.

Each level implies the ones before it: validating the metadata validates the spec first, and validating the data validates both the spec and the metadata first. Validating the spec and metadata are cheap, so they can be run continually while you edit the `data-dict.yaml`; validating the data adds a full scan and get more expensive as the size of the data increases.

Each check has a code prefixed by its level: spec checks are `S01`, `S02`, …; metadata checks `M01`, …; data checks `D01`, …. Severity is independent of level — any level can raise errors or warnings.

## Errors vs warnings

A validator reports two severities of problem: **errors** and **warnings**. The distinction is about urgency, not importance.

* An **error** means the dictionary is invalid or there's a critical mismatch between the data and dictionary. Errors will cause a production pipeline to fail, and you must fix them immediately.

* A **warning** means the dictionary is usable but the data and dictionary may have drifted apart. Warnings will not cause a production pipeline to fail, but if you're actively working on the project you should make sure to fix them.

## Spec-validation checks

| Code | Name | Sev | Description |
|------|------|-----|-------------|
| S01 | Unresolved foreign key | E | A `foreign_key` column has no `relationships` entry pointing it at a `primary_key` column. |
| S02 | Unknown table | E | A relationship references a table that is not defined in `tables`. |
| S03 | Unknown column | E | A relationship references a column that does not exist on its table. |
| S04 | Invalid join | E | A `join` expression fails to parse, or references neither one (self-join) nor two tables. |
| S05 | Unresolved conflict column | E | A name in `conflicts` is not a column on both sides of the join. |
| S06 | Inconsistent cardinality | E | The declared cardinality is inconsistent with the constraints on the joined columns (e.g. `one-to-many` whose "one" side is not `primary_key` or `unique`). |
| S07 | Wrong representation key | E | A column's data representation key is absent or wrong for its type (`enum` → `values`; `number(ordinal)`, `number(quantity)`, `date`, `datetime` → `range`; otherwise → `examples`). A `boolean` column must carry none of `values`, `range`, or `examples`. |
| S08 | Units without quantity | E | A column has `units` but its type is not `number(quantity)`. |
| S09 | Missing `$learn_more` | W | The document omits the recommended `$learn_more` key. |
| S10 | Duplicate name | E | Two column descriptors within the same table share a `name`, or two table descriptors within the dictionary share a `name`. |
| S11 | Empty name | E | A table name or a column `name` is empty. |
| S12 | Wrong value type | E | A value in `range` or `examples` does not match the column's `type` — a number type wants numbers; `string` wants strings; `date` wants an ISO 8601 date (e.g. `2024-01-31`); `datetime` wants an ISO 8601 datetime, with an offset (e.g. `2024-01-31T09:30:00Z`) unless the column has a `time_zone`, in which case it's zoneless (e.g. `2024-01-31T09:30:00`). A `range` bound may instead be `-.inf` (minimum) or `.inf` (maximum) to leave that end open, on any range type. |
| S13 | Descending range | E | A `range`'s minimum is greater than its maximum. An open bound counts as ordered only in its own place — `-.inf` as the minimum and `.inf` as the maximum; `.inf` as a minimum or `-.inf` as a maximum runs backwards. |
| S14 | Time zone without datetime | E | A column has `time_zone` but its type is not `datetime`. |
| S15 | Malformed time zone | E | A `time_zone` is not `naive`, `UTC`, or an IANA `Area/Location` name with a known area. The shape is checked, not the full tz database, so the accepted set doesn't go stale as zones are added or renamed. |
| S16 | Misplaced single-table description | W | A dictionary with exactly one table carries `label`, `description`, or `details` on that table; for a single-table dictionary these belong at the top level. |
| S17 | Malformed version | E | The top-level `version` does not give exactly one of `number`, `date`, or `hash`; its `number` is not three dot-separated numeric components (`MAJOR.MINOR.PATCH`) with an optional pre-release/build suffix; or its `date` is not a valid ISO 8601 date (`YYYY-MM-DD`). |
| S18 | Missing `$version` | E | The document omits the required top-level `$version` key. |
| S19 | Malformed assertion | E | An `assert` expression fails to parse (a syntax error in the constraint sublanguage). |
| S20 | Unknown assertion column | E | An `assert` expression, or a `COLUMNS([...])` list, references a column not present on the table. |
| S21 | Ill-typed assertion | E | An `assert` expression is syntactically valid but semantically wrong: an operator or function applied to the wrong operand type (including a column a `COLUMNS(...)` selects), a wrong function arity, a non-boolean top-level expression, more than one `COLUMNS(...)`, or a malformed `SIMILAR TO` / `COLUMNS('...')` regex. |
| S22 | Empty column selection | W | A `COLUMNS('<regex>')` in an `assert` expression matches no columns on the table (likely a typo — the assertion would hold vacuously). |
| S24 | Invalid enum values | E | An `enum`'s `values` don't describe anything the column could hold: they are empty (`[]` or `{}`), so nothing is permitted, or they are not all of the same type. Both forms are checked: the list items, and the keys of the map form. |

: {tbl-colwidths="[7,23,5,65]"}

(That each of an `enum`'s `values` is a scalar, and each label in the map form a string, is constrained structurally by the schema rather than by an `S` check; S24 covers the part the schema can't express, that they agree with each other. The `version` map's allowed keys and their value types are likewise structural, with S17 covering the rest.)

## Metadata-validation checks

| Code | Name | Sev | Description |
|------|------|-----|-------------|
| M01 | Type mismatch | E | A column's declared type is incompatible with the data. |
| M02 | Missing column | E | A column the dictionary describes is absent from the data. This applies even to columns listed by name only — listing a column that doesn't exist is an error. |
| M03 | Undocumented column | W | A column present in the data that the dictionary does not describe. This is a warning, not an error: if a production pipeline adds a column, validation should not fail, but you should document it (or at least list it by name) next time you touch the dictionary. |
| M04 | Missing source | E | A table validated against data does not declare a `source`. `source` is optional at the spec level but required here, so a validated dictionary always records where its data comes from. |
| M05 | Unreadable source | E | A table declares a `source`, but its data can't be read — the `source.parquet` file is absent, or present but not a readable Parquet file. The path is resolved relative to the dictionary file. |

: {tbl-colwidths="[7,23,5,65]"}

## Data-validation checks

| Code | Name | Sev | Description |
|------|------|-----|-------------|
| D01 | Nulls in a required column | E | A `required` or `primary_key` column contains nulls. |
| D02 | Duplicate values | E | A `unique` column contains duplicate values, or the combination of all `primary_key` columns does not uniquely identify every row. Only [comparable types](#comparable-types) are checked. Null/missing values are never counted as duplicates; for a composite primary key, a row with a null in any key column is not compared. |
| D03 | Uniqueness not verified | W | A `unique` column or `primary_key` uses a type whose values can't be reliably compared, so its uniqueness was not checked. |
| D04 | Value outside enum | E | An `enum` column contains a (non-null) value that is not one of its declared `values`. |
| D05 | Foreign key not found | E | A `foreign_key` column contains a (non-null) value that does not appear in the `primary_key` column it references. Only [comparable types](#comparable-types) are checked; null/missing values are exempt (a null foreign key references nothing). Only single-column foreign keys are checked. |
| D06 | Referential integrity not verified | W | A `foreign_key` column, or the `primary_key` it references, uses a type whose values can't be reliably compared, so the reference was not checked. |

: {tbl-colwidths="[7,23,5,65]"}

### Comparable types {#comparable-types}

The uniqueness check (D02) compares values directly, so it only runs on types whose equality is unambiguous. Which types those are depends on the data source, since each source stores values differently. Today the only source is Parquet.

For **Parquet**:

* Numbers, booleans, strings, enums, dates, and datetimes are compared by value. Decimals are compared by numeric value, regardless of how they are encoded. Floating-point values treat `-0.0` and `+0.0` as equal and all NaNs as a single value.

* JSON and BSON, whose byte representation does not determine equality (two documents can differ only in whitespace or key order and still be equal), are **not** compared. Neither is any Parquet logical type the validator does not recognize — including future types such as `VARIANT` or `GEOMETRY`.

For a non-comparable column, running the check anyway could silently miss duplicates and pass a dataset that should fail, so the check is skipped with a D03 warning instead. A composite primary key is skipped whole if any of its columns is non-comparable.

The foreign-key check (D05) is governed by the same comparability rule: the foreign-key column and the primary-key column it references are compared by the same normalized value form, so both must be comparable. If either uses a non-comparable type, the reference could silently mismatch, so the check is skipped with a D06 warning instead.
