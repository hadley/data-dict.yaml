---
title: "Installing the CLI"
---

The `data-dict` command line tool validates a `data-dict.yaml` file against the
[specification](spec.md), against a dataset's metadata, and against the data
itself; it can also draft, render, export, and translate dictionaries. See
[validation](validation.md) for what each level checks.

Every release ships prebuilt binaries, so you don't need a Rust toolchain to
install it.

## Install script

On macOS and Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/tidyverse/data-dict/releases/latest/download/data-dict-cli-installer.sh | sh
```

On Windows, in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/tidyverse/data-dict/releases/latest/download/data-dict-cli-installer.ps1 | iex"
```

The script downloads the binary for your platform, verifies its checksum, and
puts `data-dict` in `~/.cargo/bin` (`%USERPROFILE%\.cargo\bin` on Windows),
adding that directory to your `PATH` if it isn't already there. Set
`DATA_DICT_CLI_INSTALL_DIR` to install somewhere else, and
`DATA_DICT_CLI_NO_MODIFY_PATH=1` to leave your `PATH` alone.

To install a specific version, replace `latest/download` with
`download/v0.0.1` (or whichever tag you want).

Check that it worked:

```bash
data-dict --version
```

## Install with uv or pipx

`data-dict` is on [PyPI](https://pypi.org/project/data-dict/), so the Python
tool installers can manage it for you:

```bash
uv tool install data-dict
```

```bash
pipx install data-dict
```

To run it once without installing anything:

```bash
uvx data-dict validate-spec data-dict.yaml
```

Later, `uv tool upgrade data-dict` moves to the newest release, and
`uv tool install data-dict==0.0.2` pins an older one.

The wheels carry the same prebuilt binary as the install script, so there is
nothing to compile and no Rust toolchain needed. They cover macOS (Apple
silicon and Intel), Linux (x86-64 and ARM64), and Windows (x86-64). There is no
wheel for Windows on ARM64: use the install script above, which fetches the
x86-64 binary.

## Download a binary

If you'd rather not pipe a script into a shell, grab the archive for your
platform from the [releases
page](https://github.com/tidyverse/data-dict/releases/latest), unpack it, and
move the `data-dict` binary onto your `PATH`. Each archive has a matching
`.sha256` file, and every release has a combined `sha256.sum`.

The binaries aren't code-signed, so macOS quarantines archives downloaded
through a browser. If Gatekeeper refuses to run `data-dict`, clear the flag with
`xattr -d com.apple.quarantine data-dict`, or use the install script above,
which isn't affected.

Prebuilt binaries are available for:

| Platform | Target |
|----------|--------|
| macOS (Apple silicon) | `aarch64-apple-darwin` |
| macOS (Intel) | `x86_64-apple-darwin` |
| Linux (ARM64) | `aarch64-unknown-linux-musl` |
| Linux (x86-64) | `x86_64-unknown-linux-musl` |
| Windows (x86-64) | `x86_64-pc-windows-msvc` |

The Linux binaries are statically linked against musl, so they have no libc
dependency and run on any distribution, glibc or musl.

## Build from source

On any other platform, build it yourself with
[Cargo](https://rustup.rs):

```bash
cargo install --git https://github.com/tidyverse/data-dict data-dict-cli
```

The prebuilt binaries bundle the `data-dict.yaml` language server, which the
editor integrations use. A source install leaves it out unless you ask for it:

```bash
cargo install --git https://github.com/tidyverse/data-dict data-dict-cli --features lsp
```

## Uninstall

Delete the binary:

```bash
rm ~/.cargo/bin/data-dict
```

The install script also leaves a receipt in
`~/.config/data-dict-cli/data-dict-cli-receipt.json` recording what it
installed and where; you can delete that too.
