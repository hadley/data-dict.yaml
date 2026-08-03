---
name: write-data-dict
description: >-
  Create or update a data-dict.yaml file describing a dataset's tables,
  columns, types, relationships, and glossary, following the data-dict spec.
  Use when asked to document a dataset or write/update a data dictionary.
---

# Write a data dictionary

Create or update a `data-dict.yaml` file for a dataset following the
data-dict spec. Read the spec before you start by running `data-dict spec`.

The thing that matters most, and where data dictionaries most often go
wrong, are **descriptions**. Spend your effort there: a column's `type` and 
`constraints` can be inferred from the data, but its *meaning* cannot. Every 
description must say something the data itself doesn't already tell you.

**You cannot write a good data dictionary alone.** The meaning, provenance,
units, and gotchas of a dataset live in the head of whoever produced it -- not in
the data, and not in the column names. So treat this as an *interview*, not a
transcription job: surface what you don't know and ask the user, rather than
filling gaps with confident guesses. A description you invented is worse than a
question you asked -- it looks authoritative and gets trusted. **When you are not
certain a description is correct, you do not know it: ask.** Never silently write
a plausible-sounding description for a column whose meaning you had to guess.

## Steps

1.  **Discover the data and draft the skeleton.** Identify every table
    (parquet file, database table, or data-frame) in scope. For parquet files,
    run `data-dict draft <files>` to generate the starting dictionary: it
    profiles the data and writes one table per file with inferred types,
    observed ranges and examples, and a `# TODO:` comment for everything it
    can't decide -- descriptions, enum candidates, constraints, the primary
    key. Those TODOs are your work list for the steps below; delete each
    comment as you resolve it. Re-running `draft` appends tables for any new
    files without touching what you've written. To dig into a single file
    while you work, `data-dict describe <file> [column]` prints a per-column
    summary: type, distinct and missing counts, and a sketch of the values.
    For non-parquet sources, read the schema to get column names and physical
    types and write the skeleton by hand: a `data-dict.yaml` with the three
    top-level keys `tables`, `relationships`, and `glossary`.

2.  **Interview the user.** Once you know the shape of the data, work out what
    you genuinely cannot determine from it alone, and ask. This is the step most
    likely to make or break the result -- do not skip it because the column names
    look self-explanatory. They rarely are. Ask about:

    -   **What each table and row represents**, and where the data comes from,
        when it isn't obvious from the schema.
    -   **The meaning of any column you'd otherwise be guessing at** -- cryptic
        names, abbreviations, codes, or anything where you can describe the
        *shape* of the data but not what it *means*.
    -   **Units and sentinels**: what is this measured in? Are there magic values?
    -   **Which columns are trustworthy** vs. deprecated, derived, or known to be
        dirty.
    -   **Domain terms and acronyms** you don't recognise (these become glossary
        entries).
    -   **Relationships and cardinality** you can't infer from the data alone.

    Gather your questions and ask them in batches rather than one at a time, and
    rather than interrogating the user before you've done your own homework.
    Where you have a reasonable guess, offer it as a concrete option to confirm
    or correct ("`amount` looks like it's in cents -- is that right?") -- that's
    far easier to answer than an open-ended question. Record the answers directly
    into the relevant `description`, `details`, or `glossary` entry. If the user
    genuinely doesn't know, say so in `details` rather than papering over it.

3.  **Fill in each table.** For every table:

    a.  Write a `description`: a few sentences explaining what each row
        represents and where the data comes from.

    b.  Add a `source` map with the appropriate access methods (e.g.
        `parquet`, `SQL`, `R`, `Python`). You should only need to provide
        one by default.

    c.  For each column, create an entry with:

        -   `name`: must match the actual column name exactly.
        -   `label` (optional): a short, human-readable title, worth adding when
            the `name` is terse or technical (e.g. `FoodData Central ID` for
            `fdc_id`). Plain text, a few words.
        -   `type`: choose the analytical type that is *consistent with the
            physical type* of the column.
        -   `constraints`: list any that apply (`primary_key`, `required`,
            `unique`, `foreign_key`).
        -   `description` (required): a clear explanation of what the column
            contains. This is the most valuable field -- explain units,
            meaning, and anything non-obvious. Don't just restate the column
            name ("the user id" for `user_id` adds nothing). If you have nothing
            new to say, leave it blank.
        -   `range`, `examples`, or `values` as determined by the type.

    d.  Add `details` to the table or any column where there are important
        caveats, edge cases, or methodology notes that don't fit in the
        description.

4.  **Define relationships.** For every foreign key, add a relationship entry
    with `description`, `cardinality` (`one-to-many` or `many-to-one`),
    `join`, and any `conflicts` (column names that appear in both tables with
    different meanings). A self-join needs an `aliases` entry per side, naming
    the role each plays (`join: mother.otter_no = pup.pup_number` with
    `aliases: {mother: otters, pup: otters}`).

5.  **Build the glossary.** Add definitions for domain-specific terms used in
    descriptions. If a word would be unfamiliar to a new team member or an AI
    agent, define it. If you don't know what a term refers to, ask the user
    for clarification (see step 2).

6.  **Verify against the data, and against the schema.** A data dictionary
    that disagrees with the data is actively harmful, so check it:

    -   Run `data-dict validate-spec data-dict.yaml` to confirm it is
        structurally valid and passes the spec checks.
    -   Run `data-dict validate-data data-dict.yaml` to confirm every declared
        type matches the physical data. It reads each table's data through its
        `source`, so make sure every table declares one. Fix any mismatch it
        reports -- change the `type`, not the data.
    -   Confirm by inspection: do all column names match? Do primary keys
        actually uniquely identify rows? Do foreign key values exist in the
        referenced table? Are `required` columns truly non-null?

## Style

-   Use YAML block scalars (`>` for wrapping, `|` for preserving newlines)
    for multi-line text.
-   Keep descriptions concise but precise. A few sentences is usually right.
-   Order columns in the same order they appear in the underlying data.
