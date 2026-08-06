# `data-dict.yaml`

`data-dict.yaml` is a lightweight YAML specification for data dictionaries, paired with a command line application for validation. It
describes a collection of related tables — their columns, types, constraints,
relationships, and the domain vocabulary you need to understand them — in a
single file that humans and AI agents can co-author and keep in sync with your
data.

**Full documentation, including the detailed specification, lives at
[data-dict.tidyverse.org](https://data-dict.tidyverse.org).**

This repo contains two things:

* **The specification** — the prose definition of the format, in
  [`site/spec.md`](site/spec.md) (rendered at
  [data-dict.tidyverse.org](https://data-dict.tidyverse.org)).
* **The CLI** — a Rust command-line tool that validates a `data-dict.yaml`
  file against the spec and against the underlying data.

See the [examples](https://data-dict.tidyverse.org/examples/) (source in
[`site/examples/`](site/examples/)) for complete data dictionaries, or the
[overview](https://data-dict.tidyverse.org) for the motivation behind the
project.

## The CLI

The `data-dict` CLI validates dictionaries at [three levels](https://data-dict.tidyverse.org/validation.html). It can:

* Check that a file is structurally valid and internally consistent
  (`validate-spec`). Pass a file, or a directory containing a
  `data-dict.yaml` (defaults to the current directory).
* Compare a dictionary against its tables' data — column names and types
  (`validate-meta`), or values too (`validate-data`). The data is located
  through each table's `source`, so only the dictionary is passed.
* Render a dictionary as fully-resolved
  [JSON](https://data-dict.tidyverse.org/export.html), optionally with
  per-column data profiles (`export-spec` / `export-data`).
* Render a dictionary as a self-contained HTML page — a relationship
  diagram, a searchable index of tables and columns, and the glossary,
  profiled against the source data when it's present (`render`). Pass
  `--live` to serve the page instead and reload the browser as the
  dictionary and its data change.
* Summarise the columns of a Parquet file — types, distinct and missing
  counts, histograms and common values (`describe`).
* Print an embedded agent skill for reading or writing data dictionaries
  (`skill-read` / `skill-write`).
* Print the full specification (`spec`).

### Install

Build and install from source with [Cargo](https://rustup.rs):

```bash
cargo install --git https://github.com/tidyverse/data-dict data-dict-cli
```

Or clone the repo and install the local build:

```bash
git clone https://github.com/tidyverse/data-dict.git
cd data-dict
cargo install --path crates/data-dict-cli
```

This puts `data-dict` on your `PATH` (in `~/.cargo/bin`). To build without
installing, run `cargo build --release` instead — the binary is then at
`target/release/data-dict`.

### Usage

Run `data-dict` with no arguments to see the usage:

```
Usage: data-dict <COMMAND>

Commands:
  describe       Summarise the columns of a parquet file
  validate-spec  Validate a data-dict.yaml file or directory against the spec [default: .]
  validate-meta  Validate a dataset's column names and types against a data dictionary
  validate-data  Validate a dataset's values against a data dictionary
  export-spec    Render a data dictionary as fully-resolved JSON [default: .]
  export-data    Render a data dictionary as JSON with per-column data profiles
  render         Render a data dictionary as a self-contained HTML page [default: .]
  spec           Print the data-dict.yaml specification
  skill-read     Skill for reading and understanding a data dictionary
  skill-write    Skill for creating or updating a data dictionary
  help           Print this message or the help of the given subcommand(s)
```

## Development

This is a Rust workspace with three crates:

* `crates/data-dict/` — core library: YAML parsing, schema validation, lowering
  to a typed model, and semantic schema checks.
* `crates/data-dict-cli/` — thin CLI wrapper.
* `crates/data-dict-parquet/` — reads Parquet schemas and maps column types to
  data-dict types.

```bash
cargo build --workspace
cargo test --workspace
```

The rendered page's CSS and JS live in
[`crates/data-dict-cli/render/`](crates/data-dict-cli/render/) and are compiled
into the binary. A debug build run from the repo reads them from that directory
instead, so `cargo run -- render --live <dict>` reloads the browser when you
edit them — no rebuild in between.

The website is a [Quarto](https://quarto.org) project in [`site/`](site/), published automatically to [data-dict.tidyverse.org](https://data-dict.tidyverse.org) on every push to `main`.
