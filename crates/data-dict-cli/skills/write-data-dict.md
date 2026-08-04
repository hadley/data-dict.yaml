---
name: write-data-dict
description: >-
  Create or update a data-dict.yaml file describing a dataset's tables,
  columns, types, relationships, and glossary, following the data-dict spec.
  Use when asked to document a dataset or write/update a data dictionary.
---

# Write a data dictionary

Create or update a `data-dict.yaml` file for a dataset following the data-dict spec. Before you start, read the spec by running `data-dict spec`.

The thing that matters most, and where data dictionaries most often go wrong, is **descriptions**. Spend your effort there: a column's `type` and `constraints` can be inferred from the data, but its *meaning* cannot. Every description must say something the data itself doesn't already tell you.

**You cannot write a good data dictionary alone.** The meaning, provenance, units, and gotchas of a dataset live in the head of whoever produced it -- not in the data, and not in the column names. So treat this as an *interview*, not a transcription job: surface what you don't know and ask the user, rather than filling gaps with confident guesses. A description you invented is worse than a question you asked -- it looks authoritative and gets trusted. **When you are not certain a description is correct, you do not know it: ask.** Never silently write a plausible-sounding description for a column whose meaning you had to guess.

## Steps

1.  **Discover the data and context.** Identify every parquet file in scope, and look for markdown files, Word documents, and PDFs that might contain information about the data. If you don't find anything, ask the user where to look.

2.  **Draft the dictionary.** Use `data-dict draft <parquetfiles>` to generate the starting dictionary. `data-dict` will profile the data and create a draft data dictionary with one table per file, with inferred types, observed ranges and examples, and a `# TODO:` comment for everything it can't decide. Those TODOs are your work list for the steps below; delete each comment as you resolve it.

    To dig into a single file while you work, `data-dict describe <file> [column]` prints a per-column summary: type, distinct and missing counts, and a sketch of the values.

3.  **Interview the user.** Now that you know the shape of the data, work out what you don't know, and ask. This is the step most likely to make or break the result -- do not skip it because the column names look self-explanatory. Ask about:

    -   **What each table and row represents**, and where the data comes from, when it isn't obvious from the schema.
    -   **The meaning of any column you'd otherwise be guessing at** -- cryptic names, abbreviations, codes, or anything where you can describe the *shape* of the data but not what it *means*.
    -   **Units and sentinels**: what is this measured in? Are there magic values?
    -   **Which columns are trustworthy** vs. deprecated, derived, or known to be dirty.
    -   **Domain terms and acronyms** you don't recognise (these become glossary entries).
    -   **Relationships and cardinality** you can't infer from the data alone.

    Gather your questions and ask them in batches. Where you have a reasonable guess, offer it as a concrete option to confirm or correct ("`amount` looks like it's in cents -- is that right?") -- that's far easier to answer than an open-ended question. Record the answers directly into the relevant `description`, `details`, or `glossary` entry. If the user genuinely doesn't know, say so in `details` rather than papering over it.

4.  **Fill in each table.**

    a.  For every table, write a `description`: a few sentences explaining what each row represents and where the data comes from.

    b.  For each column, create an entry with:

        -   `name`: must match the actual column name exactly.
        -   `constraints`: list any that apply (`primary_key`, `required`, `unique`, `foreign_key`).
        -   `description`: a clear explanation of what the column contains. This is the most valuable field: explain units, meaning, and anything non-obvious. If you have nothing new to say, leave it blank.

    c.  Add `details` to the table or any column where there are important caveats, edge cases, or methodology notes that don't fit in the description.

5.  **Define relationships.** For every foreign key, add a relationship entry with `description`, `cardinality`, and `join`. A self-join needs an `aliases` entry per side, naming the role each plays (`join: mother.otter_no = pup.pup_number` with `aliases: {mother: otters, pup: otters}`).

6.  **Build the glossary.** Add definitions for domain-specific terms used in descriptions. If a word would be unfamiliar to a new team member or an AI agent, define it. If you don't know what a term refers to, ask the user for clarification (see step 3).

7.  **Validate.** A data dictionary that disagrees with the data is actively harmful, so check it against both the spec and the data with `data-dict validate-data data-dict.yaml`. Repeat until no problems remain.

## Style

-   Use YAML block scalars (`>` for wrapping, `|` for preserving newlines) for multi-line text.
-   Keep descriptions concise but precise. A few sentences is usually right.
