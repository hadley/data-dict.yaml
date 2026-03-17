# `dd.yaml`

`dd.yaml` is a data dictionary specification that describes a collection of related tables: their contents, constraints, connections, and the specialised vocabulary you need to understand them. It is designed to be a living document, co-written (and read) by humans and agents, that tracks your mutual understanding of a dataset.

It's designed to be lightweight, not attempting to encode every piece of metadata in a specific field, but instead has plenty of spots for free text entry.

A data dictionary has three top-level keys: 

* [`tables`](#tables) is the where the bulk of most dd.yaml files will be. It describes the tables and their fields. 
* [`relationships`](#relationships) describes the relationships between tables. It gives the details you need to safely create joins.
* [`glossary`](#glossary) provides a place to define important domain specific terms. This is a good place to write down those special words that your company loves to use.

## Tables

`tables` is a named list that describes each table in the dataset. Each table represents a rectangle of data with observations in the rows and fields/variables in the columns. Each table has the following properties:

* `description` (required): a human-readable description of the table. May contain markdown.
* `source` (required): ways to access the data.
* `fields` (required): an ordered list of field metadata.

For example:

```yaml
tables:
  food:
    description: >
      Each row is a food item in the USDA FoodData Central database.
      Includes both branded and foundation foods.
    source:
      parquet: inst/parquet/food.parquet
      R: foodbank::food
      SQL: foodbank.food
    fields:
      - name: fdc_id
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
      - name: data_type
        type: enum<foundation, branded>
        description: Whether the food is a foundation or branded food.
```

### Description

The description is a human (and agent) free text field for you to jot down any important notes that don't belong anywhere else.

### Source

`source` is a map whose keys name the access method and whose values give the location. For example:

```yaml
source:
  parquet: inst/parquet/food.parquet
  R: foodbank::food
  SQL: foodbank.food
```

The currently supported keys are:

* `parquet`: path to a Parquet file (may include globs).
* `SQL`: a (possibly schema-qualified) table name (e.g. `food` or `foodbank.food`) or a full `SELECT` query.
* `R` and `Python`: R or Python code that returns the data (e.g. `foodbank::food`, or `read.csv("food.csv", comment.char = "#")`).
* `pin`: the name of a Posit Connect pin.

This variety of source types reflects the variety of ways which you might retrieve a dataset. It's good practice to upstream as much of this processing as possible so that over time you exclusively use `parquet` or `SQL` with a table.

### Fields

Each entry in the `fields` list is a field descriptor with the following properties:

* `name` (required): column name. Must match the column name in the underlying data.
* `type`: the field's data type. Must match (approximately) the underlying data type (see [Types](#types)).
* `constraints`: a list of field-level constraints (see [Field constraints](#field-constraints)).
* `description` (required): a human-readable description of the field. Can use markdown. Can include example values. Should include surprises.
* `examples`: a list of ~5 representative values from the field. A handful of concrete examples helps LLMs understand the field far better than a description alone.

#### Types

Types capture data types at a level that makes sense for analysis, which is typically coarser than the logical types of the underlying data.

The supported types are:

* `number`: numeric values (integers or floating-point). Can be qualified with a measure in parentheses: `number(id)`, `number(ordinal)`, or `number(quantity)`. See [Measures](#measures).
* `string`: UTF-8 text strings.
* `boolean`: true/false values.
* `date`: calendar dates.
* `datetime`: date-times with timezone.
* `enum`: a string with repeated values from a finite set. The description should include a rough estimate of how many unique values there are. Inspect the table to determine the exact values.
* `enum<l1, l2, ...>`: a string with a small, known set of values listed inside angle brackets. For example, `enum<Analytical, Summed, Calculated>`. Only enumerate values when the set is small and meaningful; use plain `enum` otherwise.

#### Measures

The `number` type can be qualified with a measure in parentheses that classifies what operations are meaningful:

| Type | Can compare | Can average | Can sum | Examples |
|------------|-------------|-------------|---------|----------|
| `number(id)` | No | No | No | primary keys, foreign keys, codes |
| `number(ordinal)` | Yes | No | No | ranks, years, sequence numbers |
| `number(quantity)` | Yes | Yes | Yes | weights, counts, amounts |

#### Field constraints

The `constraints` property is a list of constraint names. The supported constraints are:

* `primary_key`: this field uniquely identifies each row. Implies `required` and `unique`.
* `required`: the field must not contain null/missing values.
* `unique`: the field's values must be distinct (no duplicates).
* `foreign_key`: the field references a primary key in another table. The specific relationship is defined in [`relationships`](#relationships).

For example: `constraints: [primary_key, required]`.

## Relationships

`relationships` is a list of join descriptors. Each entry describes how two tables are related.

* `description` (required): human-readable description of the relationship.
* `cardinality` (required): either `one-to-many` or `many-to-one`. Describes the relationship from the left table to the right table in the join expression.
* `join` (required): a join expression of the form `table1.field = table2.field`, or `table1.date >= table2.start AND table1.date <= table2.end`.
* `conflicts`: a list of field names that appear in both tables with different meanings. These fields would cause ambiguity in a join and may need to be renamed or dropped.

For example:

```yaml
relationships:
  - description: Each food belongs to one food category; each category contains many foods.
    cardinality: many-to-one
    join: food.food_category_id = food_category.id
    conflicts: [description]
```

## Glossary

`glossary` is a map from term to definition. Each entry provides a plain-language definition of a domain-specific term used in the table or field descriptions, or is likely to be used by a domain expert working with this data.

```yaml
glossary:
  foundation food: >
    A food whose nutrient and food component values are derived
    primarily by chemical analysis.
```

## Enrichment

A data dictionary can be programmatically enriched with summary statistics computed from the actual data. Enriched fields gain additional properties:

* `n_missing`: count of `NA` values (omitted when zero).
* `range`: for numeric fields, the `[min, max]` interval.
* `mean`: for numeric fields, the mean (4 significant figures).
* `n_unique`: for non-numeric fields, the number of distinct non-missing values.

The enriched dictionary also gains a `nrow` property on each table, recording the number of rows.
