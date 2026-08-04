# data-dict.yaml for VS Code

Live validation for [`data-dict.yaml`](https://data-dict.tidyverse.org/) data
dictionaries. The extension is a thin client: it launches the `data-dict`
binary as a language server (`data-dict lsp`) and shows schema and `DD0xx` lint
findings as you type.

## Prerequisites

Build the binary with the language-server feature:

```bash
cargo build -p data-dict-cli --features lsp
```

When `dataDict.server.path` is left at its default, the extension looks for a
`target/release` or `target/debug` build — first in the opened workspace, then
relative to the extension itself (so running from source via <kbd>F5</kbd> finds
this repo's build regardless of which project you open) — before falling back to
`data-dict` on your `PATH`. Set the path explicitly (an absolute path is most
reliable) if your binary lives elsewhere.

## Try it (development)

1. From `editors/vscode/`, run `npm install`.
2. Open `editors/vscode/` in VS Code and press <kbd>F5</kbd> to launch the
   Extension Development Host (this compiles via the `npm: compile` task first).
3. In the new window, open a `data-dict.yaml` file. Introduce an error — for
   example delete the `$version` key, or give an `enum` column no `values` — and
   a squiggle appears.

By default only files named `data-dict.yaml` are checked. To validate other
files (e.g. the examples under `site/examples/`), add a glob to
`dataDict.files`, such as `**/*.yaml`.

## Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `dataDict.server.path` | `data-dict` | Path to the `data-dict` executable (built with the `lsp` feature). |
| `dataDict.files` | `["**/data-dict.yaml"]` | Glob patterns for files validated as data dictionaries. |

## Commands

- **data-dict: Restart language server** — restart the server after rebuilding
  the binary.
