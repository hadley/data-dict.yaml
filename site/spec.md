# data-dict.yaml

This document describes version **0.1.0** of the `data-dict.yaml` specification.

A data dictionary has three kinds of top-level keys: `$`-prefixed metadata keys that describe the dictionary itself, descriptive keys that name and describe the dataset as a whole, and content keys that describe the data. The `$` prefix marks a key as meta, distinguishes it from content, and keeps these keys grouped at the top of the file.

The metadata keys are:

* `$version` (required): the version of the `data-dict.yaml` spec the document conforms to. Currently `0.1.0`. While the spec is pre-1.0, breaking changes are expected, but once the spec stabilises at 1.0, breaking changes will always increment at least the minor version.
* `$learn_more` (optional, but recommended): a URL where readers can learn about the `data-dict.yaml` format, so that people and tools meeting the file for the first time can find out what it is. Use <https://data-dict.tidyverse.org/>. Omitting it is valid, but a validator will emit a warning rather than an error (see [Validation](validation.md)).

The descriptive keys — `name`, `label`, `description`, and `details` — identify and document the dataset as a whole. All four are optional here, and work the same way at every level of the dictionary; see [Name, label, description & details](#name-label-description--details) for their full meaning. For the dataset, `name` is a terse identifier (e.g. `foodbank`) and `label` its human-readable title.

The dataset may also carry an optional `origin` key: a link to the code that produced it (see [Origin](#origin)). The same key is available on each table.

Every level of the dictionary — this top level, a table, a column, a struct field, a relationship — may also carry a `todo` key recording work that remains to be done; see [Todo](#todo).

In the common case of a dictionary that describes a single table, these top-level keys should be used to describe the dataset, leaving the table itself undescribed.

The content keys all hold the actual information about the data:

* [`tables`](#tables) is where the bulk of most data-dict.yaml files will be. It describes the tables and their columns.
* [`relationships`](#relationships) describes the relationships between tables. It gives the details you need to safely create joins.
* [`glossary`](#glossary) provides a place to define important domain-specific terms. This is a good place to write down those special words that your company loves to use.
* [`version`](#version) records the version of the data the dictionary describes — a version number, a date, or an opaque hash.

## Tables

`tables` is a list that describes each table in the dataset. Each table represents a rectangle of data with observations in the rows and variables in the columns. Each table has the following properties:

* `name` (required): the table's name. Used to match the table to the underlying data and to refer to it from `relationships`. Must be non-empty and unique within the dictionary.
* `label`, `description`, `details`: human-readable documentation for the table; see [Name, label, description & details](#name-label-description--details). A good table description answers at least two questions — **what's the grain?** (what does a row represent, e.g. "each row is a food item") and **what's the population?** (what's been included or filtered out, e.g. "only completed orders from 2020 onwards").
* `source`: ways to access the data. Optional at the spec level, so you can draft a dictionary before its data exists, but required to validate against data (see [Validation](validation.md)).
* `origin`: a link to the code or pipeline that produced this table's data; see [Origin](#origin).
* `columns` (required): an ordered list of column metadata.
* `constraints`: a list of table-level assertions (see [Table constraints](#table-constraints)).
* `definitions`: a list of named expressions — metrics and filters — defined on this table (see [Definitions](#definitions)).
* `todo`: work that remains on this table (see [Todo](#todo)).

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
* `todo`: work that remains on this column (see [Todo](#todo)).

Some properties only apply to certain types:

* `units`: the unit of measurement, for `number(quantity)` columns only (see [Measures](#measures)).
* `time_zone`: the time zone, for `datetime` columns only (see [Time zones](#time-zones)).

Each column also needs to describe some representative values, using `values`, `range`, or `examples` as its type requires. See [Representative values](#representative-values) for details.

A column may also be listed with only its `name` and no `type`. This acknowledges the column without describing it and you should use it for columns that you don't care about but don't want flagged as undocumented. Such a column makes no claims about its contents, so it's never checked, but it must still exist in the data. Such columns should not be used in analysis or exposed in user interfaces.

#### Name, label, description & details

`name`, `label`, `description`, and `details` document a dataset, table, column, or definition, from terse to expansive. `name` is the only required field; all others are optional. They mean the same thing at every level:

* `name` identifies the thing. For a table, definition, or column it's an identifier matched against the underlying data, so it must be non-empty and unique (a table within the dictionary, a column within its table). For the dataset it's just a short, machine-friendly id (e.g. `foodbank`) with no constraints.
* `label` is a short, human-readable title, useful when the `name` is terse or technical (e.g. `FoodData Central ID` for `fdc_id`). Plain text (no markdown), typically a few words, it stands in for the `name` in user interfaces.
* `description` contains the most important information about the item, like known limitations or a surprising derivation compared to its `name`.
* `details` contains anything else that might be useful to know, e.g. assumptions about potential unknowns, or background on how the data was collected or constructed.

The primary difference between `description` and `details` is their treatment in user interfaces: `description` is shown in full, whereas you'll often need to click to see all of the `details`. Both `description` and `details` accept markdown.

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
* `enum`: a string column with repeated values from a known set. The allowed values are listed in the `values` property, and are always strings.
* `list(element_type)`: an ordered sequence of zero or more elements of the given type (see [List element types](#list-element-types)).
* `struct`: a structured record with named fields documented in the required `fields` property (see [Struct fields](#struct-fields)).

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

#### List element types

The element type in `list(element_type)` may be any type: `string`, `number`, `number(id)`, `number(ordinal)`, `number(quantity)`, `boolean`, `date`, `datetime`, `enum`, `struct`, or another `list(...)`, nested to any depth. The same properties that apply to a column of that type apply when it is used as a list element type — `values` for `enum`, `fields` for `struct`, `units` for `number(quantity)`, `time_zone` for `datetime`, and so on. For nested lists those properties follow the *innermost* element type: a `list(list(number(quantity)))` declares `units` and a `range`, and they describe the innermost values.

```yaml
- name: tags
  type: list(string)
  examples: [nature, outdoor, urban, photography, wildlife]

- name: categories
  type: list(enum)
  values: [food, drink, dessert]

- name: temperature_grid
  type: list(list(number(quantity)))
  units: °C
  range: [-40, 60]

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
    - name: price
      type: number(quantity)
      units: USD
      range: [0.99, 999.99]
```

#### Struct fields

A `struct` column may include a `fields` property — an ordered list of field descriptors. A field descriptor is a reduced column descriptor: it carries the properties that name, type, and document the field (e.g. `name`, `type`, `description`, `details`, `todo`, etc). It doesn't yet support: `label`, `display`, and `constraints`.

A field may itself be `list(...)` or `struct` (with its own `fields`), allowing deep nesting.

```yaml
- name: address
  type: struct
  fields:
    - name: street
      type: string
      examples: [123 Main St, 456 Oak Ave, 789 Elm Dr]
    - name: city
      type: string
      examples: [Portland, Austin, Chicago]
    - name: zip
      type: string
      examples: ["97201", "78701", "60601"]
    - name: country
      type: enum
      values: [US, CA, MX]
```

A rule about a field's values is written as an assertion on the enclosing column (or table), using [field access](expressions.md#field-access):

```yaml
- name: address
  type: struct
  constraints:
    - assert: LENGTH(address.zip) = 5
  fields: ...
```

#### Representative values

Most typed columns carry at least one of the following three properties to represent the data they contain, chosen by the column's type. The exceptions are `boolean` (values are always `true`/`false`) and `struct` (whose fields carry their own).

A numeric or temporal column may give both `range` and `examples`, since the two say different things: the extremes the column reaches, and what a typical value looks like. One is usually enough — reach for the pair only when the extremes alone would mislead, as in a column whose maximum is a rare outlier. Which one the type *requires* is unchanged; the other is optional.

* `values`: the allowed values for an `enum` column. Can be a list (`[M, F, U]`) when values are self-explanatory, or a map (`{M: Male, F: Female, U: Unknown}`) when values need labels. The values themselves must be **strings**, and there must be at least one of them; in the map form the labels must be strings too. (`boolean` columns implicitly have `values: [true, false]`, no need to explicitly include it.)
* `range`: a two-element list `[min, max]` giving the inclusive minimum and maximum *observed* in the column. Like `examples`, it describes the data rather than constraining it: nothing is validated against it, and a value outside it is not an error. To constrain a column, write an [`assert`](expressions.md). Required for the ordered numeric and temporal types: `number(ordinal)`, `number(quantity)`, `date`, and `datetime`. Optional on the other numeric types, `number` and `number(id)`, where it may accompany their `examples`. Both elements must match the column's type, and the minimum must not exceed the maximum.

    Either bound may be left open with negative infinity (`-.inf`) for the minimum or positive infinity (`.inf`) for the maximum. An open bound says the true extent is unknown or constantly moving, as in a daily export whose date column always runs up to the present. If you leave a bound open, make sure to describe the range in prose in the column's `description`. `.inf` here means the bound is *open*, not that the column was observed to contain an infinity — a column really can hold one, but a range is descriptive, so the two readings are not worth distinguishing. `.nan` is not a bound at all, and is rejected in `examples` too (S12).
* `examples`: a list of ~5 representative values from the column. Required for all other types: `string`, `number`, and `number(id)`. Optional on the ordered numeric and temporal types, where it may accompany their `range`. Each example must match the column's type, so a `string` column's examples need quoting whenever they read as numbers (`['02134', '94110']`). A handful of concrete examples helps LLMs understand the column far better than a description alone. For instance, knowing that an id column holds `[1, 2, 3, 4, 5]` versus `[10000, 1235452, 234234]` tells a very different story. A good baseline is to select 5 evenly spaced values along the sorted unique values, and then add any particularly surprising values as you encounter them. Example values are evocative rather than exhaustive; long values will be truncated and restricted values will have plausible, but fake data.

`boolean` columns are the exception to this rule because they can only contain `true`, `false`, and (if not required) `null`.

For `list(element_type)` columns, the same properties apply but describe the element values, not the lists themselves. The mapping follows the element type — the innermost one, for nested lists: `values` for `list(enum)`, `range` for `list(number(ordinal))`, `list(number(quantity))`, `list(date)`, and `list(datetime)`, `examples` for `list(string)`, `list(number)`, and `list(number(id))`, and no representative values for `list(boolean)` or `list(struct)` (same as their scalar counterparts). Each property means the same thing it would for a scalar column of the element type — for instance, `range` on a `list(number(quantity))` column gives the minimum and maximum element value observed across all lists.

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
* `required`: the column does not contain null/missing values. On a `list` element this only implies non-null, not non-empty.
* `unique`: the column's values are distinct (no duplicates). Null/missing values are exempt — a `unique` column may contain multiple nulls, and nulls are never treated as duplicates.

`unique`, `primary_key`, `foreign_key` are not valid on `list` or `struct` columns, and constraints belong to columns; fields within a `struct` can't carry them (see [Struct fields](#struct-fields)).

An assertion is a map with an `assert` key holding a boolean expression that must be true for every row, plus an optional `description` and an optional [`language`](#other-languages):

```yaml
columns:
  - name: postcode
    type: string
    constraints:
      - required
      - assert: LENGTH(postcode) <= 10
```

Bare names in the expression refer to columns and definitions in the same table. A field of a `struct` column is referenced with a dot (`address.zip`). See [Assertions](#assertions) below for a summary, and [Expressions](expressions.md) for the full language.

Note that `values` and `range` (see [Types](#types)) already express membership and bounds constraints — `values` restricts an `enum` to its listed set, and `range` bounds an ordered column — so you don't need an assertion to repeat them.

### Table constraints

A table's `constraints` property is a list of assertions, using exactly the same form as a [column assertion](#column-constraints): a map with an `assert` key and an optional `description` and [`language`](#other-languages). The only difference is scope — a table constraint isn't tied to a single column, so it's the natural home for rules that span columns:

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

An `assert` expression is a single-table boolean expression in data-dict's small SQL-like [expression language](expressions.md) — or [written in another language](#other-languages) and read into it. Most are row-level: evaluated against every row, with the constraint holding unless the expression is *false* for some row. Bare names refer to columns or definitions in the table.

Expressions use SQL's three-valued logic, so an expression is `true`, `false`, or `null` (unknown) for a given row — a comparison involving a null operand is `null`, not `false` (`LENGTH(postcode) <= 10` is `null` when `postcode` is null). Following SQL's `CHECK` semantics, a row **passes** when the expression is `true` **or** `null`, and only a `false` result is a violation. So an assertion never doubles as a null check: `LENGTH(postcode) <= 10` constrains the length of the values that *are* present but says nothing about missing ones. Pair it with the `required` constraint (or an explicit `IS NOT NULL`) when the column must also be non-null.

Assertions state what must be **true**, so conditional rules are written as implications, e.g. `NOT(q3) OR q4 IS NOT NULL`.

Assertions are deliberately **single-table**: an expression sees only the columns of one table. There are no subqueries — cross-table rules belong in [`relationships`](#relationships). Within a table an expression has two grains available: a column reference is read one row at a time, and an aggregate folds a column over every row into one value. The two can be mixed, so `value <= 2 * MIN(value)` is a legitimate rule; an assertion that uses no aggregate is checked row by row, and a violation can name the offending row. Aside from `NOW()`, assertions are deterministic: the same data always gives the same result.

The language offers the SQL operators you'd expect — comparisons, `AND`/`OR`/`NOT`, `IS NULL`, `BETWEEN`, `IN`, `LIKE`, `SIMILAR TO`, `CASE`, and arithmetic — over column references, numeric, string, boolean, and `NULL` literals, plus a handful of string (`LENGTH`, `LOWER`, `UPPER`, `TRIM`, `STARTS_WITH`, `ENDS_WITH`), numeric (`ABS`, `ROUND`, `FLOOR`, `CEIL`, `MOD`), date/time (`NOW()`, `interval(<n>, <unit>)`), and aggregate (`MIN`, `MAX`, `SUM`, `AVG`, `COUNT`, `ROW_COUNT`, `COUNT_DISTINCT`, `ANY`, `ALL`) functions. A `COLUMNS(...)` expression applies one predicate to many columns at once:

```yaml
constraints:
  # Every q4–q8 answer is present whenever q3 is true.
  - assert: NOT(q3) OR COLUMNS('q[4-8]') IS NOT NULL
    description: q4–q8 must be answered when q3 is true.
```

[Expressions](expressions.md) documents the language in full: every operator and function with its input and output types, precedence, the `COLUMNS(...)` forms, the type rules a validator enforces, and the grammar.

#### Writing an expression in another language {#other-languages}

Expressions are written in data-dict's own [expression language](expressions.md) by default. An author who already thinks in another language can write the rule there instead, and say so with a `language` key beside the `assert` or `expr` it applies to:

```yaml
columns:
  - name: postcode
    type: string
    constraints:
      - assert: nchar(postcode) <= 10
        language: r
```

`language` names a language, not a dialect and not a package: `data-dict` (the default) or `r`. `r` covers the spellings base R, dplyr/stringr, and data.table. Each use (e.g.`nchar` and `str_length`) are read, and nothing has to say which one was meant. Writing `language: data-dict` is always allowed and means exactly what leaving the key out means. The set is closed and grows with the spec, so a `language` a validator can't read is rejected outright rather than left to mean something later.

A table can hold rules written in different languages side by side:

```yaml
tables:
  - name: survey
    constraints:
      - assert: end_date >= start_date
      - assert: nchar(postcode) <= 10
        language: r
```

Whatever language an expression is written in, it is read into the one expression language. The language an author writes in changes how a rule is *spelled*, never what it *means*: `nchar(postcode) <= 10` under `language: r` is the rule `LENGTH(postcode) <= 10`, [three-valued logic](expressions.md#truth-and-null) and all, and not whatever R would do with that line.

A construct whose meaning in its own language differs from the reading it is given. R's `round` rounds halves to even where the language rounds them away from zero. This still reads, and carries a note saying how the two differ ([S36](validation.md#spec-validation-checks)). A construct the language has no equivalent for at all is an error ([S35](validation.md#spec-validation-checks)): the rule has to be rewritten, in either language.

The dictionary keeps what the author wrote. The `assert` text is quoted back verbatim wherever a rule is named, so the line a problem points at is the line in the file. IF the data-dict spelling is wanted too, e.g. to compare two rules written in different languages, or to rewrite a dictionary in one, `data-dict translate --target data-dict` prints it, and the [export document](export.md#assertion) carries it beside the original.

### Definitions

A table's `definitions` property is a list of named expressions — the metrics, filters, and derivations that consumers should reuse rather than reinvent. Where a [constraint](#table-constraints) states something that must hold of the data, a definition states something that can be computed from it.

Each entry is a map with:

* `name` (required): the definition's name. Must be non-empty and unique within the table. Definitions and columns share a namespace: a definition's name must not match any column name in the same table.
* `expr` (required): an expression in the [expression language](expressions.md), or [written in another language](#other-languages) and read into it. Unlike an assertion, it need not be boolean.
* `language`: the language `expr` is written in; see [Writing an expression in another language](#other-languages). Omitted, it is data-dict's own.
* `label`, `description`, `details`: human-readable documentation for the definition; see [Name, label, description & details](#name-label-description--details).
* `todo`: a note of work that remains before the definition is complete; see [Todo](#todo).

The kind of a definition is read off the expression's type and [shape](expressions.md#shapes):

* A boolean, `row`-shape expression is a **filter**, a named segment of the table's rows.
* A `agg`- or `const`-shape expression is a **metric**, a single value summarising the rows it's evaluated against.
* Any other `row`-shape expression is a **derived** value, with one value for each row, which can be used to generate a column. A filter is also a derived value.

The expression language has no grouping of its own: a metric is defined over whatever rows the consumer evaluates it against, and any grouping — SQL's `GROUP BY`, dplyr's `.by =`, a dashboard's drill-down — is up to the host environment to supply.

```yaml
tables:
  - name: orders
    definitions:
      - name: net_revenue
        description: Realized revenue excluding returned orders.
        expr: SUM(CASE WHEN status_cd = 90 THEN 0 ELSE order_total END)
      - name: is_enterprise
        description: For historical reasons, the Enterprise segment also includes Mid-Market-3.
        expr: tile_size IN ('Mid-Market-3', 'Enterprise-1', 'Enterprise-2', 'Enterprise-3')
```

A definition's expression may reference the table's columns and other definitions in the same table. References between definitions must not be circular: following the references from any definition must eventually reach only columns.

A [`COLUMNS(...)`](expressions.md#selecting-multiple-columns) selection may appear only in a filter where each selected column is tested in turn and the results combined with `AND`, exactly as in a constraint.

Definitions are checked when the spec is validated — the expression must parse, type-check, and reference only the table's columns and definitions — but the metadata and data levels never evaluate them.

## Relationships

`relationships` is a list of join descriptors. Each entry describes how two tables are related.

* `join` (required): a join expression of the form `table1.column = table2.column`, or `table1.date >= table2.start AND table1.date <= table2.end`.
* `cardinality` (required): either `one-to-one`, `one-to-many`, or `many-to-one`. Describes the relationship from the left side to the right side of the join expression.
* `description`: human-readable description of the relationship. Only needed if it's not clear from the context.
* `conflicts`: a list of column names that appear on both sides of the join with different meanings. These fields would cause ambiguity in a join and may need to be renamed or dropped.
* `aliases`: a map from alias to table name, naming the role each side of the join plays. See [aliases](#aliases).
* `todo`: work that remains on this relationship (see [Todo](#todo)).

For example:

```yaml
relationships:
  - join: food.food_category_id = food_category.id
    cardinality: many-to-one
    conflicts: [description]
```

### Aliases

A join brings together two sets of rows. Usually they come from two different tables, so the table names are enough to tell the sides apart. When they don't, `aliases` gives each side its own name:

```yaml
relationships:
  - join: mother.otter_no = pup.pup_number
    aliases:
      mother: otters
      pup: otters
    cardinality: one-to-many
    description: Links a female otter to her dependent pup's own record.
```

Within a `join`, a name before the `.` resolves first as an alias declared by that relationship, then as a table name. An alias is scoped to the relationship that declares it, and it must not have the same name as a table in the dictionary. Every alias must name a table that exists, and every alias declared should be used by the join. Self-joins must use aliases.

Prefer aliases that name the role a side plays (`mother`/`pup`, `manager`/`report`) over positional names like `left`/`right`; they make the join expression readable on its own.

Aliases are also allowed, but not required, when two tables are joined more than once and each join means something different. Naming the roles says which is which:

```yaml
relationships:
  - join: flights.origin = origin_airport.faa
    aliases: {origin_airport: airports}
    cardinality: many-to-one
  - join: flights.dest = dest_airport.faa
    aliases: {dest_airport: airports}
    cardinality: many-to-one
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

## Todo

`todo` records work that remains before the dictionary is complete: a description still to be written, a type to confirm with a domain expert, a constraint you suspect but haven't verified. Its value is a single string saying what's left to do. When several tasks are outstanding, write them as one note — a literal block scalar (`|`) keeps a bulleted list readable:

```yaml
- name: status
  type: string
  examples: [active, pending, closed]
  todo: |
    - Add a `description`.
    - Looks like an enum — confirm the full set of `values` and switch the `type`.
```

`todo` may appear at every level that describes something: the top level (the dataset), a table, a column, a struct field, a relationship, and a definition. Put each note on the thing it's about, so the work travels with its subject.

Every remaining `todo` is reported when the spec is validated (see S31 in [validation](validation.md)), so a dictionary announces its own unfinished work each time it's checked. It's reported as a warning rather than an error, so an unfinished dictionary can still be validated against its data — but you should resolve every `todo` before you consider the dictionary finished.
