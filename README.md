# `data-dict.yaml`

`data-dict.yaml` is a data dictionary specification that describes a collection of related tables: their contents, constraints, connections, and the specialised vocabulary you need to understand them. It is designed to be a living document, co-written by humans and agents, that tracks your understanding of a dataset as it evolves.

`data-dict.yaml` is designed to be lightweight. It doesn't attempt to precisely describe every possible type of metadata in a machine readable way. Instead it focuses on precisely recording the most important components, leaving the remainder to plain text fields that require a human or agent to interpret. This means that `data-dict.yaml` doesn't itself do **data cleaning**, but it is a useful complement to tools that do.

You can read the details of the spec in [spec.md](spec.md), or dive in by looking at a few examples:

* [dabstep](examples/dabstep.yaml)
* [elevators](examples/elevators.yaml)
* [foodbank](examples/foodbank.yaml)
* [loan-application](examples/loan-application.yaml)
* [otters](examples/otters.yaml)

## Why `data-dict.yaml`?

There have been many previous attempts to encode data dictionaries in structured text. What makes `data-dict.yaml` different? Why revisit this problem now?

* The costs of creating a data dictionary are lower than ever before because AI agents can automate much of the boilerplate, including porting documentation from existing unstructured formats (e.g. `.doc`, `.html`, `.pdf`).
* The benefits of creating a data dictionary are higher, because AI agents need the context that currently exists only in your head. As a very pleasant side-effect, this also helps your human colleagues, particularly those who are newer to your organisation.
* LLMs change what it means for something to be machine readable. While we explicitly encode the most important structures, we can leave the more unusual quirks to free-form text.
* Unlike previous data dictionaries, we assume data is stored in parquet files or database tables. This means that many parsing details are out of scope, radically simplifying the spec.
* The cost of describing the data semantics in multiple places (i.e. `data-dict.yaml` and data transformation code) is lower because an AI agent can easily keep both in sync.

## Inspirations

Here are a few of the resources that guided the design of `data-dict.yaml`:

* [Data management in large-scale education research](https://datamgmtinedresearch.com/document#document-dataset)
* [Frictionless data](https://datapackage.org/standard/table-schema)
* [Hex's semantic modelling](https://learn.hex.tech/docs/connect-to-data/semantic-models/semantic-authoring/modeling-specification)
* [Snowflake's semantic views](https://docs.snowflake.com/en/user-guide/views-semantic/overview)

It's worth noting that while semantic models influenced the design of `data-dict.yaml`, it is not a **[semantic model](semantic-models.md)**. This means it doesn't think about dimensions or metrics, because that distinction reflects intended use, not the data itself. It's primarily designed to support data scientists, not data analysts.

## Missing features

`data-dict.yaml` is currently just a spec — it has no accompanying tooling to validate dictionaries or perform other useful tasks. We plan to implement these in the future:

* **Data validation**: There's currently no way to verify that a dataset and spec are consistent. We plan to provide a tool that ensures that your yaml file is correctly structured and consistent with the corresponding data. This allows a `data-dict.yaml` to serve as a data contract, ensuring that data meets agreed-upon expectations.

* **Large tables**: A standalone `data-dict.yaml` is not designed for hundreds of tables or hundreds of columns. We also plan to provide tools that allow you to aggregate multiple dictionaries and index larger data catalogs.

* **User facing documentation**: There's currently no way to turn your `.yaml` file into attractive HTML documentation of your data. If you've put the time into maintaining an accurate data dictionary, we want to make it easy to turn it into a beautiful website that you can share with your colleagues.
