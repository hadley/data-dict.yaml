# Write a data dictionary

Create or update a `data-dict.yaml` file for a dataset following the
[spec](https://github.com/hadley/data-dict.yaml/blob/main/spec.md). Read the spec before you start.

## Steps

1.  **Discover the data.** Identify every table (parquet file, database table,
    or data-frame) in scope. For each table, read the schema to get column
    names and physical types.

2.  **Create the skeleton.** Start a `data-dict.yaml` with the three
    top-level keys: `tables`, `relationships`, and `glossary`.

3.  **Fill in each table.** For every table:

    a.  Write a `description`: a few sentences explaining what each row
        represents and where the data comes from.

    b.  Add a `source` map with the appropriate access methods (e.g.
        `parquet`, `SQL`, `R`, `Python`). You should only need to provide
        one by default.

    c.  For each column, create an entry with:

        -   `name`: must match the actual column name exactly.
        -   `type`: choose the appropriate analytical type (`number`,
            `string`, `boolean`, `date`, `datetime`, `enum`,
            `enum<l1, l2, ...>`). For numbers, add a measure when possible:
            `number(id)`, `number(ordinal)`, or `number(quantity)`.
        -   `constraints`: list any that apply (`primary_key`, `required`,
            `unique`, `foreign_key`).
        -   `description` (required): a clear explanation of what the column
            contains.
        -   `examples`: ~5 representative values, chosen by selecting evenly
            spaced values from the sorted unique values. Omit for enums with
            listed levels.

    d.  Add `details` to the table or any column where there are important
        caveats, edge cases, or methodology notes that don't fit in the
        description.

4.  **Define relationships.** For every foreign key, add a relationship entry
    with `description`, `cardinality` (`one-to-many` or `many-to-one`),
    `join`, and any `conflicts` (column names that appear in both tables with
    different meanings).

5.  **Build the glossary.** Add definitions for domain-specific terms used in
    descriptions. If a word would be unfamiliar to a new team member or an AI
    agent, define it. If you don't know what a term refers to, ask the user
    for clarification.

6.  **Verify.** Check the dictionary against the actual data:

    -   Do all column names match?
    -   Are types consistent with the physical data types?
    -   Do primary keys actually uniquely identify rows?
    -   Do foreign key values exist in the referenced table?
    -   Are `required` columns truly non-null?

## Style

-   Use YAML block scalars (`>` for wrapping, `|` for preserving newlines)
    for multi-line text.
-   Keep descriptions concise but precise. A few sentences is usually right.
-   For `enum` types with a small known set of values, list them inline:
    `enum<M, F, U>`. Use the `description` to explain what each level means
    using a markdown list.
-   Order columns in the same order they appear in the underlying data.
