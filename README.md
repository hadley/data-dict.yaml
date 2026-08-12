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

The `data-dict` CLI validates dictionaries at [three levels](https://data-dict.tidyverse.org/validation.html), and can also render, export, and describe them. 

Run `data-dict` with no arguments to see the usage:

```
Usage: data-dict <COMMAND>

Commands:
  describe       Summarise the columns of a parquet file
  draft          Generate a starting data-dict.yaml from parquet files
  validate-spec  Validate a data-dict.yaml file or directory against the spec
  validate-meta  Validate a dataset's column names and types against a data dictionary
  validate-data  Validate a dataset's values against a data dictionary
  export-spec    Render a data dictionary as fully-resolved JSON
  export-data    Render a data dictionary as JSON with per-column data profiles
  render         Render a data dictionary as a self-contained HTML page
  translate      Translate a dictionary's assertions into R, Python, or SQL
  spec           Print the data-dict.yaml specification
  skill-read     Skill for reading and understanding a data dictionary
  skill-create   Skill for creating a data dictionary
  help           Print this message or the help of the given subcommand(s)
```


### Install

Every release ships prebuilt binaries for macOS, Linux, and Windows. On macOS
and Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/tidyverse/data-dict/releases/latest/download/data-dict-cli-installer.sh | sh
```

On Windows, in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/tidyverse/data-dict/releases/latest/download/data-dict-cli-installer.ps1 | iex"
```

This puts `data-dict` on your `PATH` (in `~/.cargo/bin`). You can also download
an archive straight from the [releases
page](https://github.com/tidyverse/data-dict/releases/latest), or build from
source with [Cargo](https://rustup.rs):

```bash
cargo install --git https://github.com/tidyverse/data-dict data-dict-cli
```

See [the install
page](https://data-dict.tidyverse.org/install.html) for the other options, including
the supported platforms and how to uninstall.

## Development

This is a Rust workspace with four crates:

* `crates/data-dict/` — core library: YAML parsing, schema validation, lowering
  to a typed model, and semantic schema checks.
* `crates/data-dict-cli/` — thin CLI wrapper.
* `crates/data-dict-parquet/` — reads Parquet schemas and maps column types to
  data-dict types.
* `crates/data-dict-lsp/` — language server, compiled into the CLI behind its
  `lsp` feature.

```bash
cargo build --workspace
cargo test --workspace
cargo run -p data-dict-cli -- ...
```

The rendered page's CSS and JS live in
[`crates/data-dict-cli/render/`](crates/data-dict-cli/render/) and are compiled
into the binary. A debug build run from the repo reads them from that directory
instead, so `cargo run -- render --live <dict>` reloads the browser when you
edit them — no rebuild in between.

The website is a [Quarto](https://quarto.org) project in [`site/`](site/), published automatically to [data-dict.tidyverse.org](https://data-dict.tidyverse.org) on every push to `main`.

### Releases

Releases are built by [`dist`](https://opensource.axo.dev/cargo-dist/),
configured in [`dist-workspace.toml`](dist-workspace.toml). To cut one, bump
`workspace.package.version` in [`Cargo.toml`](Cargo.toml), then tag and push:

```bash
git tag v0.1.0
git push origin v0.1.0
```

`.github/workflows/release.yml` then builds the binaries for every supported
platform, generates the installers and checksums, and publishes a GitHub
release. The workflow is generated, not hand-written: after changing
`dist-workspace.toml`, re-run `dist init` (or `dist generate`) with the `dist`
version pinned in that file, and commit the result. `dist plan` shows what a
release would produce without building it.
