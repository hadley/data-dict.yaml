---
title: "data-dict.yaml"
---

`data-dict.yaml` is a data dictionary specification that describes a collection of related tables: their contents, constraints, connections, and the specialised vocabulary you need to understand them. It is designed to be a living document, accessible to both humans and agents, that tracks your understanding of a dataset as it evolves.

`data-dict.yaml` is designed to be lightweight. It doesn't attempt to precisely describe every possible type of metadata in a machine-readable way. Instead it focuses on precisely recording the most important components, leaving the remainder to plain text fields that require a human or agent to interpret. This means that `data-dict.yaml` doesn't itself do **data cleaning**, but it is a useful complement to tools that do.

You can read the details of the spec in [the specification](spec.md), or dive in by looking at a few [examples](examples/index.qmd):

* [dabstep](examples/dabstep.qmd)
* [elevators](examples/elevators.qmd)
* [foodbank](examples/foodbank.qmd)
* [loan-application](examples/loan-application.qmd)
* [otters](examples/otters.qmd)

## Why use `data-dict.yaml`?

A data dictionary is one place to record everything you know about your data, making it accessible to everyone who works with it. `data-dict.yaml` helps you do this with two coupled components, a specification and a command-line interface (CLI).

The specification is:

* Open and supported by Posit, a public benefit corporation with a mission to create free and open-source software for data science, scientific research, and technical communication. We don't yet have a formal governance model, but in the meantime, you're very welcome to propose changes and additions in the [issues](https://github.com/tidyverse/data-dict/issues).

* Built by data people, for data people. We understand the challenges data folks face, and we've designed the spec to address them directly. This means including pieces that other dictionaries omit: support for metadata like data version and column units, descriptions of the relationships between datasets, and a glossary for domain- and team-specific terminology.

* A plain text YAML 1.2 document. Compared to other formats like Excel and PDF, YAML is easily diffable so that you can see how it changes over time. Compared to formats like JSON or XML, it's easily editable by humans, not just machines.

* Assumes that data lives in parquet and database tables. Parquet is an open format designed to store data compactly while maximising performance on modern hardware. This keeps the scope of the specification tight (no need to describe the many wrinkles of CSV) and encourages best practices for data storage.

The CLI, `data-dict`, is the other half of the story. It enforces the standard and provides a growing set of useful tools.

* It is open source and free, and does not require any hosted services.

* It's self-contained, so that all you need to install is a single binary. This makes it straightforward to create, check, and maintain a data dictionary on both your local machine and in CI.

* It validates the data contract, ensuring the data and dictionary stay consistent. Validation is actively expanding, and currently covers variable names, types, ranges, and constraints such as uniqueness.

* Future tooling will include the ability to turn a `data-dict.yaml` into a browsable interactive website, simulate data from a dictionary, generate a skeleton dictionary from a Parquet file or database table, and search across multiple `data-dict.yaml` files.

## Why now? (_cough_ AI _cough_)

There have been many previous attempts to encode data dictionaries in structured text. What makes `data-dict.yaml` different, and why revisit this problem now? The answer is AI. We believe AI fundamentally changes both the costs and benefits of a data dictionary:

* The costs of creating a data dictionary are lower than ever, because AI agents can automate much of the boilerplate, including porting documentation from existing unstructured formats (`.doc`, `.html`, `.pdf`). An agent can also surface questions about aspects of the data that are unspecified or ambiguous.
* The benefits are higher, because AI agents need the context that currently exists only in your head. Providing it via a data dictionary helps your AI tools work more accurately.
* The schema can be simpler because LLMs change what it means for something to be machine-readable. You only need to explicitly encode the most important structures, leaving more unusual quirks to free-form text.

## When should you use `data-dict.yaml`?

`data-dict.yaml` is designed to support a wide range of scenarios. You might use it:

* Before you have any data, as a way to be concrete about your goals and expectations. In the future, we plan to support [data simulation](https://github.com/tidyverse/data-dict/issues/20) so that you can generate a dummy dataset compatible with your data dictionary.
* When you first encounter a new dataset, as a way of recording what you learn as you learn it. This can be particularly useful with public datasets, where an LLM may know more about the data than you do.
* Retrospectively, when you've already spent considerable time with a dataset. Creating a `data-dict.yaml` gets the knowledge out of your head and makes it accessible to your human and AI collaborators. You can get started quickly by asking an LLM to extract what it knows based on the code you've written so far.
* When working with regularly updated data produced by someone else. Maybe you get updates from collaborators via email, or your data engineering team has a habit of not announcing changes to variable definitions. `data-dict.yaml`'s ability to validate data against the spec ensures you're never surprised when the data changes.

## Inspirations

Here are a few of the resources that guided the design of `data-dict.yaml`:

* [Data management in large-scale education research](https://datamgmtinedresearch.com/document#document-dataset)
* [Frictionless data](https://datapackage.org/standard/table-schema)
* [Hex's semantic modelling](https://learn.hex.tech/docs/connect-to-data/semantic-models/semantic-authoring/modeling-specification)
* [Snowflake's semantic views](https://docs.snowflake.com/en/user-guide/views-semantic/overview)
* [Soda's contract language](https://docs.soda.io/reference/contract-language-reference)
* [dbt tests](https://docs.getdbt.com/docs/build/data-tests?version=1.12)
* [Data Package Standard](https://datapackage.org)
* [Brain Imaging Data Structure](https://bids.neuroimaging.io)
* [Data Documentation Initiative](https://ddialliance.org)

It's worth noting that while semantic models influenced the design of `data-dict.yaml`, it is not a **[semantic model](semantic-models.md)**. It doesn't model dimensions or metrics, because that distinction reflects intended use, not the data itself. `data-dict.yaml` is primarily designed to support data scientists, not data analysts.

Additionally, while terminology is still evolving, the "semantic" in semantic models is typically interpreted narrowly, focussing on structural semantics — what's needed for queries to return consistent values — rather than what the data actually _means_.

## Missing features

`data-dict.yaml` currently ships a validator (see the [CLI](https://github.com/tidyverse/data-dict#readme)) that checks a dictionary against the spec and against the underlying data. We plan to add more tooling in the future:

* **User-facing documentation**: There's currently no way to turn your `.yaml` file into attractive HTML documentation. If you've put the time into maintaining an accurate data dictionary, we want to make it easy to turn it into a website you can share with colleagues.

* **Large tables**: A standalone `data-dict.yaml` is not designed for hundreds of tables or hundreds of columns. We plan to provide tools that allow you to aggregate multiple dictionaries and index larger data catalogs.

* **Export**: Export your data dictionary to other formats such as CSV, Excel, and Google Sheets.
