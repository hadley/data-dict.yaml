---
name: create-data-dict
description: >-
  Create an initial `data-dict.yaml` file describing a dataset's tables,
  columns, types, relationships, and glossary, following the data-dict spec.
  Use when asked to document a dataset or write/update a data dictionary.
---

# Create a data dictionary

This skill teaches you how to create a `data-dict.yaml` file for a dataset, following the data-dict spec. Before you start, read the spec by running `data-dict spec`.

**You cannot write a good data dictionary alone.** The meaning, provenance, units, and gotchas of a dataset live in the head of whoever produced it -- not in the data, and not in the column names. So treat this as an *exploration* and an *interview*, not a transcription job: surface what you don't know and ask the user, rather than filling gaps with confident guesses. A description you invented is worse than a question you asked -- it looks authoritative and gets trusted. **When you are not certain a description is correct, you do not know it: ask.** Never silently write a plausible-sounding description for a column whose meaning you had to guess.

## Steps

1.  **Discover the data and context.** Identify every Parquet file in scope, and look for Markdown files, Word documents, and PDFs that might contain information about the data. If you don't find anything, ask the user where to look.

2.  **Draft the dictionary.** Use `data-dict draft <parquet-files>` to generate the starting dictionary. `data-dict` will profile the data and create a draft data dictionary with one table per file. It will add inferred types, observed ranges and examples, and a `todo` entry for everything it can't decide. Those todos are your work list for the steps below; delete each one as you resolve it.

    To dig into a single file or column while you work, run `data-dict describe <file> [column]` -- it prints a per-column summary: type, distinct and missing counts, and a sketch of the values.

3.  **Record your own todos.** Now that you know the shape of the data, work out what you don't know, and adds to the todo items. This is the step most likely to make or break the result -- do not skip it because the column names look self-explanatory. Read the existing documentation, and where not clear ask about:

    -   **What each table and row represents**, and where the data comes from..
    -   **The meaning of any column you'd otherwise be guessing at** -- cryptic names, abbreviations, codes, or anything where you can describe the *shape* of the data but not what it *means*.
    -   **Units and sentinels**: what is this measured in? Are there magic values?
    -   **Which columns are trustworthy** vs. deprecated, derived, or known to be dirty.
    -   **Domain terms and acronyms** you don't recognise (these become glossary entries).
    -   **Relationships and cardinality** you can't infer from the data alone.

    Where you have a reasonable guess, offer it as a concrete option to confirm or correct ("`amount` looks like it's in cents -- is that right?") -- that's far easier to answer than an open-ended question. Record every open question as a `todo` on the exact relationship, table, column, or field it concerns (`todo: Confirm whether amount is in cents or dollars.`).

4.  **Resolve the todos.** Work systematically through every open question, asking the user for clarification as needed. As the answers come in, record each one in the relevant `description`, `details`, or `glossary` entry and delete the `todo` it settles. If the user genuinely doesn't know, leave the `todo` open.

5.  **Define relationships.** For every foreign key, add a relationship entry with `description`, `cardinality`, and `join`. A self-join needs an `aliases` entry per side, naming the role each plays (`join: mother.otter_no = pup.pup_number` with `aliases: {mother: otters, pup: otters}`).

6.  **Build the glossary.** Add definitions for domain-specific terms used in descriptions. If a word would be unfamiliar to a new team member or an AI agent, define it. If you don't know what a term refers to, ask the user for clarification (see step 3).

7.  **Validate.** Check your work by running `data-dict validate-data data-dict.yaml`, repeating until no problems remain. Every remaining `todo` is reported as a warning, so the dictionary isn't finished while any are left -- but don't resolve a `todo` by deleting it or by guessing; resolve it by getting the answer. It's ok to finish with todos still open if the user has work to do.

## Style

-   Use YAML block scalars (`>` for wrapping, `|` for preserving newlines) for multi-line text.
-   Keep descriptions concise but precise. A few sentences is usually right.
