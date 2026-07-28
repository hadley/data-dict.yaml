# data-dict.yaml

This document describes version **0.1.0** of the `data-dict.yaml` specification.

A data dictionary has three kinds of top-level keys: `$`-prefixed metadata keys that describe the dictionary itself, descriptive keys that name and describe the dataset as a whole, and content keys that describe the data. The `$` prefix marks a key as meta, distinguishes it from content, and keeps these keys grouped at the top of the file.

The metadata keys are:

* `$version` (required): the version of the `data-dict.yaml` spec the document conforms to. Currently `0.1.0`. While the spec is pre-1.0, breaking changes are expected, but once the spec stabilises at 1.0, breaking changes will always increment at least the minor version.
* `$learn_more` (optional, but recommended): a URL where readers can learn about the `data-dict.yaml` format, so that people and tools meeting the file for the first time can find out what it is. Use <http://data-dict.tidyverse.org/>. Omitting it is valid, but a validator will emit a warning rather than an error (see [Validation](validation.md)).

The descriptive keys — `name`, `label`, `description`, and `details` — identify and document the dataset as a whole. All four are optional here, and work the same way at every level of the dictionary; see [Name, label, description & details](#name-label-description--details) for their full meaning. For the dataset, `name` is a terse identifier (e.g. `foodbank`) and `label` its human-readable title.

The dataset may also carry an optional `origin` key: a link to the code that produced it (see [Origin](#origin)). The same key is available on each table.

In the common case of a dictionary that describes a single table, these top-level keys should be used to describe the dataset, leaving the table itself undescribed.

The content keys all hold the actual information about the data:

* [`tables`](#tables) is where the bulk of most data-dict.yaml files will be. It describes the tables and their columns.
* [`relationships`](#relationships) describes the relationships between tables. It gives the details you need to safely create joins.
* [`glossary`](#glossary) provides a place to define important domain-specific terms. This is a good place to write down those special words that your company loves to use.
* [`version`](#version) records the version of the data the dictionary describes — a version number, a date, or an opaque hash.

## Tables

`tables` is a list that describes each table in the dataset. Each table represents a rectangle of data with observations in the rows and variables in the columns. Each table has the following properties:

* `name` (required): the table's name. Used to match the table to the underlying data and to refer to it from `relationships`. Must be non-empty and unique within the dictionary.
* `label`, `description`, `details`: human-readable documentation for the table; see [Name, label, description & details](#name-label-description--details).
* `source`: ways to access the data. Optional at the spec level, so you can draft a dictionary before its data exists, but required to validate against data (see [Validation](validation.md)).
* `origin`: a link to the code or pipeline that produced this table's data; see [Origin](#origin).
* `columns` (required): an ordered list of column metadata.
* `constraints`: a list of table-level assertions (see [Table constraints](#table-constraints)).

For example:

```yaml
tables:
  - name: food
    label: Foods
    description: >
      Each row is a food item in the USDA FoodData Central database.
      Includes both branded and foundation foods.
    source:
      parquet: inst/parquet/food.parquet
    columns:
      - name: fdc_id
        label: FoodData Central ID
        type: number(id)
        constraints: [primary_key]
        description: Unique identifier for the food item.
        examples: [167512, 174231, 325871, 534109, 715322]
      - name: description
        type: string
        constraints: [required]
        description: Full text description of the food.
        examples: [Hummus, Egg rolls, Cheese spread, Grapes, Pickle relish]
      - name: food_category_id
        type: number(id)
        constraints: [foreign_key]
        description: Links to the food_category table.
        examples: [9, 11, 14, 18, 25]
      - name: data_type
        type: enum
        values: [foundation, branded]
        description: Whether the food is a foundation or branded food.
```

### Source

`source` describes how to access the table's data. It's a map whose keys describe the access method and whose values give the location. Currently the only supported key is `parquet`:

```yaml
source:
  parquet: inst/parquet/food.parquet
```

* `parquet`: path to a Parquet file (may include globs). Relative paths are resolved relative to the dictionary file.

Parquet is the only source `data-dict` can currently validate against, so it's the only one the spec defines. We expect to add more access methods in the future — most importantly `SQL` (a schema-qualified table name such as `foodbank.food`, or a full `SELECT` query), and likely others such as R, Python, and Posit Connect pins.

`source` is optional while you're only validating the spec, letting you sketch a table before its data exists. But the metadata and data levels validate the dictionary against real data, so every table they check must declare a `source` whose file exists and is readable.

### Origin

`origin` is an optional link to the code that produced the data — the script, pipeline, or repository a reader can follow to see how the data was built. It's a single string holding either a URL or a path:

```yaml
# A URL...
origin: https://github.com/example/foodbank/blob/main/data-raw/food.R

# ...or a path, resolved relative to the dictionary file.
origin: data-raw/food.R
```

A path points at a script alongside the dictionary; a URL points anywhere, such as a repository or the entry point of a workflow tool like `targets`. The validator treats `origin` as a reference for a human or agent to follow — it never fetches a URL or checks that a path exists.

`origin` may be given for the whole dataset (at the top level) or for an individual table. Use the dataset level when a single pipeline produces everything, and the table level when tables are built by different scripts. If several scripts feed one table, link the directory or repository rather than listing them all.

### Columns

Each entry in the `columns` list is a column descriptor. Columns are matched to the underlying data by `name`, so the order in which you list them does not need to match the column order in the data.

Each descriptor has the following properties:

* `name` (required): column name. Used to match the descriptor to a column in the underlying data. Must be non-empty and unique within a table.
* `label`, `description`, `details`: human-readable documentation for the column; see [Name, label, description & details](#name-label-description--details).
* `type`: the column's data type (see [Types](#types)). Should match (approximately) the underlying data type. Optional — see below.
* `constraints`: a list of column-level constraints (see [Column constraints](#column-constraints)).
* `display`: controls whether the column should appear in user-facing output (see [Display](#display)).

Some properties only apply to certain types:

* `units`: the unit of measurement, for `number(quantity)` columns only (see [Measures](#measures)).
* `time_zone`: the time zone, for `datetime` columns only (see [Time zones](#time-zones)).

Each column also needs to describe some representative values, using exactly one of `values`, `range`, or `examples`. See [Representative values](#representative-values) for details.

A column may also be listed with only its `name` and no `type`. This acknowledges the column without describing it and you should use it for columns that you don't care about but don't want flagged as undocumented. Such a column makes no claims about its contents, so it's never checked, but it must still exist in the data. Such columns should not be used in analysis or exposed in user interfaces.

#### Name, label, description & details

`name`, `label`, `description`, and `details` document a dataset, table, or column, from terse to expansive. They mean the same thing at every level:

* `name` identifies the thing. For a table or column it's an identifier matched against the underlying data, so it must be non-empty and unique (a table within the dictionary, a column within its table). For the dataset it's just a short, machine-friendly id (e.g. `foodbank`) with no constraints. It's the only one of the four that is ever required.
* `label` is a short, human-readable title, useful when the `name` is terse or technical (e.g. `FoodData Central ID` for `fdc_id`). Plain text (no markdown), typically a few words, it stands in for the `name` in user interfaces.
* `description` is a short summary, typically a few sentences or at most a paragraph. May contain markdown, and is displayed in user interfaces. A good table description answers two questions — **what's the grain?** (what does a row represent, e.g. "each row is a food item") and **what's the population?** (what's been included or filtered out, e.g. "only completed orders from 2020 onwards").
* `details` is a free-text note of any length: the place to carefully record everything else, such as assumptions about potential unknowns, known weak spots, surprising calculations, and how the data was collected or constructed.

Every field but `name` is optional at every level.

#### Display

The optional `display` property controls whether a column should appear in user-facing output. Currently, the only supported value is `restricted`:

```yaml
- name: ssn
  type: string
  display: restricted
  examples: ["000-00-0000", "123-45-6789"]
```

A restricted column must be excluded from default user interfaces and other user-facing output, including tables, plots, and downloads. (And its examples should not include real data). We can't guarantee this protection, but we hope it will steer agents (and humans!) away from showing it by default.

The primary use case is **personally identifiable information (PII)** — columns containing data such as names, email addresses, phone numbers, social security numbers, or other details that identify an individual. More broadly, `display: restricted` applies to any sensitive, confidential, or secret data that should not be surfaced by default.

#### Types

Types capture data types at a level that makes sense for analysis, which is typically coarser than the logical types of the underlying data.

The supported types are:

* `number`: numeric values (integers or floating-point). Can be qualified with a measure in parentheses: `number(id)`, `number(ordinal)`, or `number(quantity)`. See [Measures](#measures).
* `string`: UTF-8 text strings.
* `boolean`: true/false values.
* `date`: calendar dates, written as ISO 8601 strings (`YYYY-MM-DD`, e.g. `2024-01-31`).
* `datetime`: date-times, written as ISO 8601 strings. Without a `time_zone` they carry an offset (e.g. `2024-01-31T09:30:00Z`); with a `time_zone` they're written zoneless and interpreted in that zone (see [Time zones](#time-zones)).
* `enum`: a column with repeated values from a known set. The allowed values are listed in the `values` property.

#### Measures

The `number` type can be qualified with a measure in parentheses that classifies what operations are meaningful:

| Type | Can compare | Can average | Can sum | Examples |
|------------|-------------|-------------|---------|----------|
| `number(id)` | No | No | No | primary keys, foreign keys, codes |
| `number(ordinal)` | Yes | No | No | ranks, years, sequence numbers |
| `number(quantity)` | Yes | Yes | Yes | weights, counts, amounts |

A `number(quantity)` column can also declare its `units`: a free-text string naming the unit of measurement, such as `kg`, `USD`, or `seconds`. Units are only meaningful for quantities — they're how you tell apart two columns that share a `range` but measure different things — so `units` is an error on any other type.

```yaml
- name: mass
  type: number(quantity)
  units: g
  range: [0, 5000]
```

#### Representative values

Every type has some way of representing the data it contains: an exhaustive set of values, a range, or a handful of examples. Each such column carries exactly one of the following three properties, determined by the column's `type`:

* `values`: the allowed values for an `enum` column. Can be a list (`[M, F, U]`) when values are self-explanatory, or a map (`{M: Male, F: Female, U: Unknown}`) when values need labels. The values themselves must be scalars (string, number, or boolean); in the map form the labels must be strings. (`boolean` columns implicitly have `values: [true, false]`, no need to explicitly include it.)
* `range`: a two-element list `[min, max]` giving the inclusive minimum and maximum *observed* in the column. Like `examples`, it describes the data rather than constraining it — a value outside the range will generate a warning, not a validation error. Used for the ordered numeric and temporal types: `number(ordinal)`, `number(quantity)`, `date`, and `datetime`. Both elements must match the column's type, and the minimum must not exceed the maximum.

    Either bound may be left open with negative infinity (`-.inf`) for the minimum or positive infinity (`.inf`) for the maximum. An open bound says the true extent is unknown or constantly moving, as in a daily export whose date column always runs up to the present. If you leave a bound open, make sure to describe the range in prose in the column's `description`.
* `examples`: a list of ~5 representative values from the column. Used for all other types: `string`, `number`, and `number(id)`. Each example must match the column's type. A handful of concrete examples helps LLMs understand the column far better than a description alone. For instance, knowing that an id column holds `[1, 2, 3, 4, 5]` versus `[10000, 1235452, 234234]` tells a very different story. A good baseline is to select 5 evenly spaced values along the sorted unique values, and then add any particularly surprising values as you encounter them.

`boolean` columns are the exception to this rule because they can only contain `true`, `false`, and (if not required) `null`.

#### Time zones

A `datetime` column can declare its `time_zone`, which says how to interpret its values as moments in time. The value is either an [IANA time zone name](https://en.wikipedia.org/wiki/List_of_tz_database_time_zones) or the sentinel `naive`:

* A named zone — `UTC`, `America/New_York`, `Europe/Paris`, and so on — means the column records instants in time, displayed in that zone. `UTC` is the usual choice for timestamps stored as instants.
* `naive` means the column records wall-clock date-times with no associated zone, so the same value can refer to different instants in different places. Use it for local times whose offset is unknown or irrelevant.

A named zone is either `UTC` or an IANA `Area/Location` name whose `Area` is one of `Africa`, `America`, `Antarctica`, `Arctic`, `Asia`, `Atlantic`, `Australia`, `Europe`, `Indian`, `Pacific`, or `Etc` (e.g. `America/New_York`, `Etc/GMT+5`). Validation checks this shape and the `Area` — enough to catch ambiguous abbreviations like `PST` or `EST` — but does not check the full location against a time zone database, so the accepted set doesn't go stale as zones are added or renamed.

Time zones are only meaningful for date-times, so `time_zone` is an error on any other type. Omit `time_zone` when the zone is unknown or doesn't matter.

```yaml
- name: observed_at
  type: datetime
  time_zone: UTC
  description: A running log; the newest timestamp advances with every export.
  range: [2020-01-01T00:00:00, .inf]
```
NB: when `time_zone` is present, write the column's `range` as plain, zoneless date-times; they're interpreted in the declared zone.


#### Column constraints

The `constraints` property is a list of constraints. Each entry is either a **structural constraint** (a bareword naming a structural or relational fact about the column) or an **assertion** (a map carrying an expression that must hold for the data).

The structural constraints are:

* `primary_key`: the set of columns with the `primary_key` constraint uniquely identifies each row. Implies `required` and `unique`.
* `foreign_key`: the column references a primary key in another table (or in the current table, if a self-join). The specific relationship is defined in [`relationships`](#relationships). Validating the data checks that every value appears in the referenced primary key (see D05/D06 in [validation](validation.md)).
* `required`: the column does not contain null/missing values.
* `unique`: the column's values are distinct (no duplicates). Null/missing values are exempt — a `unique` column may contain multiple nulls, and nulls are never treated as duplicates.

An assertion is a map with an `assert` key holding a boolean expression that must be true for every row, plus an optional `description`:

```yaml
columns:
  - name: postcode
    type: string
    constraints:
      - required
      - assert: LENGTH(postcode) <= 10
```

Bare column names in the expression refer to columns of the same table, so a column assertion may relate its column to any sibling. See [Assertions](#assertions) for the expression grammar.

Note that `values` and `range` (see [Types](#types)) already express membership and bounds constraints — `values` restricts an `enum` to its listed set, and `range` bounds an ordered column — so you don't need an assertion to repeat them.

### Table constraints

A table's `constraints` property is a list of assertions, using exactly the same form as a [column assertion](#column-constraints): a map with an `assert` key and an optional `description`. The only difference is scope — a table constraint isn't tied to a single column, so it's the natural home for rules that span columns:

```yaml
tables:
  - name: survey
    constraints:
      - assert: end_date >= start_date
        description: A contract can't end before it starts.
      - assert: NOT(q3) OR (q4 IS NOT NULL AND q5 IS NOT NULL)
        description: If q3 is true, q4 and q5 must be answered.
```

Table constraints can only carry assertions; the structural barewords (`primary_key`, `unique`, …) live on columns.

### Assertions

An `assert` expression is a single-table, row-level boolean expression written in a small SQL-like sublanguage. It is evaluated against every row, and the constraint holds unless the expression is *false* for some row. Bare names refer to columns of the table.

Expressions use SQL's three-valued logic, so an expression is `true`, `false`, or `null` (unknown) for a given row — a comparison involving a null operand is `null`, not `false` (`LENGTH(postcode) <= 10` is `null` when `postcode` is null). Following SQL's `CHECK` semantics, a row **passes** when the expression is `true` **or** `null`, and only a `false` result is a violation. So an assertion never doubles as a null check: `LENGTH(postcode) <= 10` constrains the length of the values that *are* present but says nothing about missing ones. Pair it with the `required` constraint (or an explicit `IS NOT NULL`) when the column must also be non-null.

Assertions are deliberately **per-row and single-table**: an expression sees only the columns of one row at a time. There are no aggregates and no subqueries — cross-table rules belong in [`relationships`](#relationships), and the per-row restriction keeps assertions cheap to check. Aside from `NOW()`, they are also deterministic: the same row always gives the same result. `NOW()` is the one exception — it reads the current time, so an assertion that uses it (for example, a freshness check like `some_date >= NOW() - interval(2, weeks)`) can pass one day and fail the next even though the data hasn't changed.

The supported grammar is:

* **Comparison:** `=`, `!=` / `<>`, `<`, `<=`, `>`, `>=`
* **Logic:** `AND`, `OR`, `NOT`, parentheses for grouping
* **Null tests:** `IS NULL`, `IS NOT NULL`
* **Membership:** `x BETWEEN lo AND hi`, `x IN (...)`, `x NOT IN (...)`
* **Pattern matching:** `LIKE` / `NOT LIKE` (with `%` and `_` wildcards) and `SIMILAR TO` / `NOT SIMILAR TO` (regular expressions). A `SIMILAR TO` pattern is an [RE2](https://github.com/google/re2/wiki/Syntax) regular expression, anchored so it must match the whole string (as in DuckDB, and unlike PostgreSQL's `SIMILAR TO`, it does not use `%`/`_` wildcards).
* **Conditional:** `CASE WHEN ... THEN ... ELSE ... END`
* **String functions:** `LENGTH`, `LOWER`, `UPPER`, `TRIM`, `STARTS_WITH`, `ENDS_WITH`
* **Numeric functions & arithmetic:** `ABS`, `ROUND`, `FLOOR`, `CEIL`, `MOD`, and `+ - * /`
* **Date/time:** `NOW()`, `interval(<n>, <unit>)`, and arithmetic between dates and intervals. The `<unit>` is one of `seconds`, `minutes`, `hours`, `days`, or `weeks` — all fixed-length. Calendar units (`months`, `years`) are deliberately excluded: they're non-uniform (adding a month lands on a different number of days depending on the date, and clamps at month ends), which would make an assertion's meaning depend on the calendar. Express a rough month as `interval(30, days)` if you need it.
* **Column selection:** `COLUMNS(...)`, to apply one predicate to many columns at once (see below)

Operator precedence and associativity follow standard SQL (so, from tightest to loosest: arithmetic, then comparisons, then `NOT`, `AND`, `OR`); use parentheses when in doubt.

Keywords and function names are case-insensitive, as in SQL — `AND`, `and`, `LENGTH`, and `length` are all accepted. Column names are the exception: unlike SQL's unquoted identifiers, they are case-sensitive, matched exactly against the column names in the data (which may themselves differ only by case), consistent with how `name` is matched everywhere else. Value comparisons (`=`, `LIKE`, `SIMILAR TO`, …) are case-sensitive too.

Assertions state what must be **true**, so conditional rules are written as implications, e.g. `NOT(q3) OR q4 IS NOT NULL` or `NOT(q3 AND q4 IS NULL)`.

Operands are column names, literals, and function results. The literal forms are:

* **Numbers** — an integer (`42`) or decimal (`3.14`); a leading `-` is unary minus.
* **Strings** — single-quoted (`'ABC'`); double a quote to include one (`'O''Brien'`).
* **Booleans** — `TRUE` or `FALSE`.
* **Null** — `NULL`.

There is no dedicated date/datetime literal: a date or datetime is written as a single-quoted ISO 8601 string (`'2024-01-31'`, `'2024-01-31T09:30:00Z'`) and compared against a `date`/`datetime` column, or produced by `NOW()` and `interval(...)`.

An `assert` expression as a whole must be **boolean** — a bare non-boolean column (e.g. a number) is not a valid assertion on its own; compare or test it (`qty > 0`, `postcode IS NOT NULL`). Functions take fixed arities: the string functions `LENGTH`, `LOWER`, `UPPER`, `TRIM` take one argument; `STARTS_WITH`, `ENDS_WITH`, `MOD` take two; `ABS`, `FLOOR`, `CEIL` take one; `ROUND` takes one or two (the second is the number of decimal places); `NOW()` takes none; and `interval(<n>, <unit>)` takes an integer and a unit keyword.

#### Selecting multiple columns

To apply the same predicate to a group of columns without repeating it, an assertion may use a `COLUMNS(...)` expression — a simple subset of [DuckDB's `COLUMNS`](https://duckdb.org/docs/current/sql/expressions/star). The supported forms select columns by:

* `COLUMNS(*)`: all columns in the table.
* `COLUMNS('<regex>')`: columns whose name matches the regular expression. The pattern is an [RE2](https://github.com/google/re2/wiki/Syntax) regular expression matched unanchored (a partial match, as in DuckDB), so `COLUMNS('q')` selects every column whose name contains a `q`; anchor it with `^`/`$` to match the whole name. (This is the one place a regex is unanchored — `SIMILAR TO` anchors.)
* `COLUMNS([a, b, c])`: an explicit list of column names.

The enclosing expression is evaluated once per selected column, and the assertion holds only when it is true for **every** selected column (the results are combined with `AND`). At most one `COLUMNS(...)` may appear in an `assert` expression, so there's no ambiguity about how multiple selections would combine. Because the predicate is applied to each selected column, every one of them must fit the way the expression uses it — `LENGTH(COLUMNS('name_.*'))` requires that each matched column is a string, just as a bare column reference would. A `COLUMNS('<regex>')` whose pattern matches no column is almost always a typo, so it's reported as a warning (the assertion would otherwise hold vacuously). So:

```yaml
constraints:
  # Every q4–q8 answer is present whenever q3 is true.
  - assert: NOT(q3) OR COLUMNS('q[4-8]') IS NOT NULL
    description: q4–q8 must be answered when q3 is true.
  # No column anywhere in the table is null.
  - assert: COLUMNS(*) IS NOT NULL
```

The lambda form (`COLUMNS(c -> ...)`) and the star modifiers (`EXCLUDE`, `REPLACE`, `RENAME`) are **not** supported.

## Relationships

`relationships` is a list of join descriptors. Each entry describes how two tables are related.

* `join` (required): a join expression of the form `table1.column = table2.column`, or `table1.date >= table2.start AND table1.date <= table2.end`.
* `cardinality` (required): either `one-to-one`, `one-to-many`, or `many-to-one`. Describes the relationship from the left table to the right table in the join expression.
* `description`: human-readable description of the relationship. Only needed if it's not clear from the context.
* `conflicts`: a list of column names that appear in both tables with different meanings. These fields would cause ambiguity in a join and may need to be renamed or dropped.

For example:

```yaml
relationships:
  - join: food.food_category_id = food_category.id
    cardinality: many-to-one
    conflicts: [description]
```

## Glossary

`glossary` is a map from term to definition. Each entry provides a plain-language definition of a domain-specific term that appears in the table or column descriptions or is likely to be used by a domain expert working with this data.

```yaml
glossary:
  foundation food: >
    A food whose nutrient and food component values are derived
    primarily by chemical analysis.
```

## Version

`version` records the version of the data this dictionary describes, so people and tools can tell two snapshots of the data apart and know which one a given dictionary goes with. (This is distinct from `$version`, which records the version of the *spec* the document conforms to.)

`version` is optional, but if present it should appear at the top of the file. It's a map with exactly one of three keys, which names both the kind of version and its value:

* `number`: a hand-curated version number with three dot-separated numeric components, optionally followed by a pre-release (`-…`) and/or build (`+…`) suffix, such as `1.2.0` or `1.2.0-rc.1`.
* `date`: a release date in ISO 8601 form (`YYYY-MM-DD`), such as `2024-01-31`, for data refreshed on a schedule.
* `hash`: an opaque identifier, such as `a1b2c3d`, derived from the data itself.

If you use a `number`, we recommend [semantic versioning](https://datapackage.org/recipes/data-package-version/): increment the first component for incompatible changes, the second for backwards-compatible additions, and the third for backwards-compatible fixes.

`data-dict` checks that exactly one key is present, that a `number` has three dot-separated numeric components (with an optional suffix), and that a `date` is a valid ISO 8601 date, but otherwise treats the version as opaque.

```yaml
version:
  date: 2024-01-31
```
