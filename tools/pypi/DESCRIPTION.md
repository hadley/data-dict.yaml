# data-dict

[`data-dict.yaml`](https://data-dict.tidyverse.org) is a lightweight YAML
specification for data dictionaries: it describes a collection of related
tables — their columns, types, constraints, relationships, and glossary — in one
file that both people and tools can read. `data-dict` is the command line tool
that validates such a file against the specification, against a dataset's
metadata, and against the data itself.

## Install

```bash
uv tool install data-dict-yaml
```

The installed command is `data-dict`. The wheels ship the prebuilt binary, so
there is nothing to compile and no Rust toolchain to install. They cover macOS
(Apple silicon and Intel), Linux (x86_64 and aarch64, glibc and musl alike),
and Windows (x86_64).

## Use

```bash
data-dict validate-spec data-dict.yaml
data-dict validate-data data-dict.yaml
```

`validate-spec` checks the dictionary against the specification,
`validate-meta` additionally checks it against a dataset's schema, and
`validate-data` also checks the rows. Diagnostics point at the offending line
and explain what was expected. The tool can also draft a dictionary from a
dataset, render one to HTML, export it as JSON, and translate its expressions
to R, Python, or SQL. Run `data-dict --help` for the full list.

See <https://data-dict.tidyverse.org> for the specification, the list of
validation checks, and the other install options. Source and issues:
<https://github.com/tidyverse/data-dict>.
