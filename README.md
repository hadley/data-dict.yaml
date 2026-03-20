# `dd.yaml`

`dd.yaml` is a data dictionary specification that describes a collection of related tables: their contents, constraints, connections, and the specialised vocabulary you need to understand them. It is designed to be a living document, co-written (and read) by humans and agents, that tracks your mutual understanding of a dataset.

It's designed to be lightweight, not attempting to encode every piece of metadata in a specific field, but instead has plenty of spots for free text entry.

You can read the details of the spec in [spec.md], or dive in by looking at a few examples:

* [dabstep](examples/dabstep.yaml)
* [elevators](examples/elevators.yaml)
* [foodbank](examples/foodbank.yaml)
* [loan-application](examples/loan-application.yaml)
* [otters](examples/otters.yaml)

## Why `dd.yaml`?

There have been many previous attempts to encode data dictionaries in structured text. What makes `dd.yaml` special and why did we decide to revisit this problem?

* The costs of creating a structured data dictionary are lower because an LLM can automate much of the boilerplate and can port documentation from existing unstructured formats (e.g. `.doc`, `.html`, `.pdf`).
* The benefits of creating a structured data dictionary are higher, because AI agents need the context that currently exists only in your head. And this will also help out your human colleagues!
* LLMs change what it means for something to be machine readable. While we explicitly encode the most important structures, we can leave more unusual quirks to free-form text.
* Unlike previous data dictionaries, we assume data is stored in parquet files or database tables. This means that we declare the details of parsing arbitrary files out of scope, radically simplifying the spec.
* The cost of describing the semantics of a dataset in two places (i.e. `dd.yaml` and data transformation code) is lower because an AI agent can easily keep both in sync.

## What `dd.yaml` doesn't do

There are a few things that `dd.yaml` deliberately doesn't do in order to keep scope tight:

* A toolkit for accurately describing the full **data cleaning** journey. `dd.yaml` is primarily useful for describing the mostly-clean mostly-tidy datasets at the end of a data engineering pipeline. 

* `dd.yaml` is not a **data validation** tool. You can check that the spec and the data are aligned, but it’s up to you to fix inconsistencies to ensure that your spec has not deviated from reality.

* `dd.yaml` is not a **[semantic model](semantic-models.md)**. It doesn’t classify columns as dimensions or metrics, because that distinction reflects intended use, not the data itself. Data scientists need to understand what the data means, not have views predefined for them.

There are other things that `dd.yaml` doesn't do **yet**, but are on the roadmap:

* **Large tables**: Standalone `dd.yaml` are not designed for hundreds of tables or hundreds of columns. Our plan is to also provide a service that allows you to index much larger data catalogs.

* **Spec validation**: There's currently no way to verify that a dataset and spec are consistent. We plan to provide a tool that ensures that your yaml file is correctly structured and consistent with the underlying dataset.

* **User facing documentation**: There's no way currently to turn your `.yaml` file into attractive HTML documentation of your data. You've put the time into maintaining an accurate data dictionary, we want to make it easy to turn into a beautiful website that you can share with your colleagues.
