# data-dict.yaml

`data-dict.yaml` is a lightweight YAML specification for data dictionaries, paired with a command line application for validation. It describes collections of related tables — their columns, types, constraints, relationships, and glossary. The main deliverable is a self-contained rust CLI called `data-dict`.

The repo contains:

- `site/spec.md`: the full specification (v0.1.0)
- `README.md`: project overview, CLI install/build instructions, and a pointer to the site.
- `site/`: the [Quarto](https://quarto.org) website published to data-dict.tidyverse.org. Holds the spec and design docs (`spec.md`, `semantic-models.md`), as well as example data dictionaries synced from other repos (see `examples.R`, which holds the list and is driven by both workflows: `Rscript examples.R sync` in `.github/workflows/update-examples.yaml`, `Rscript examples.R render` in `.github/workflows/publish-site.yaml`). Built and deployed by `.github/workflows/publish-site.yaml`; the rendered example pages in `site/examples/rendered/` are generated at publish time, not committed.
- `crates/`: Rust workspace (see crate architecture below)
- `r/`: the `datadict` R package — a thin wrapper that downloads the released binary into `tools::R_user_dir("datadict", "cache")` (`dd_install()`) and shells out to it (`dd_validate_data()`). Base R plus processx, which `dd_run()` uses to capture the binary's interleaved output while optionally echoing it; test it with `R CMD check`, pointing `DATA_DICT` at a local build.
- `tools/pypi/`: `make_wheels.py` repacks the release archives into PyPI wheels (one per target, carrying the prebuilt binary as a wheel script), run from `.github/workflows/publish-pypi.yml`, which fires on completion of the release workflow (or by hand, with a tag) and downloads the archives from the GitHub release. Stdlib only; test with `python3 -m unittest discover -s tools/pypi`.
- `schema.yaml`: JSON Schema for structural validation of data dictionary files (`schema-field.yaml` holds the recursive struct-field descriptor it references)
- `dist-workspace.toml`: release config for [`dist`](https://opensource.axo.dev/cargo-dist/). It generates `.github/workflows/release.yml` — never hand-edit that file; change the config and re-run `dist generate`. Install docs live in `site/install.md` and the README.

## Code principles

* Reserve comments for explaining why, not what or how. Default to no comment. Before writing one, check it isn't already said by the item's name, its type, its doc comment, or the line below it — if so, drop it.
* Don't comment on the historical evolution of the code (what it used to do, what changed) or speculate about future work ("we'll handle X later", "grows as Y is added"). Comment only on the code as it stands.
* Keep doc comments to what a caller can't infer from the signature (invariants, units, edge cases, spec rules); don't restate the name.
* User facing code should be accompanied by a test.

## Spec and implementation must stay in sync

The spec (`site/spec.md` + validation details in `site/validation.md`) and the implementation (the crates + `schema.yaml`) are two views of the same thing and must never drift apart.

- **New features start in the spec, and REQUIRE human sign-off.** This is the single most important rule in this file. Any new feature is a two-phase process with a hard stop between the phases:
    1. **Write the spec.** Draft and iterate the change in `site/spec.md` *only*. Do not touch `schema.yaml`, the crates, the tests, or any other file in this phase.
    2. **Stop and get an explicit "yes" from a human on the spec text.** Asking clarifying questions is not sign-off. Presenting a plan is not sign-off. You must show the human the actual spec wording and wait for them to explicitly approve *that wording* before writing a single line of implementation. If you are unsure whether you have approval, you do not have approval — ask again.

  Only after that explicit yes do you implement (`schema.yaml`, crates, tests). Starting implementation before the human has signed off on the spec is a process violation, even if the feature itself is fine.
- **Implementation refinements flow back to the spec.** If you discover during implementation that the spec is wrong, incomplete, or ambiguous, update `site/spec.md` to match what you actually built.
- **Touch one, check the other.** Whenever you change the spec, double-check the implementation still matches; whenever you change the implementation, update the spec. A change to either is incomplete until both agree.

## Commands

```bash
# Build
cargo build --workspace
cargo build --workspace --all-targets   # includes tests, examples, benches

# Test
cargo test --workspace
cargo test -p data-dict                 # single crate
cargo test -p data-dict spec            # tests matching "spec" in data-dict crate

# Format and lint (run before committing Rust changes)
cargo fmt --all
cargo clippy --workspace --all-targets

# Validate a file
cargo run -p data-dict-cli -- validate-spec site/examples/otters.yaml
```

To review/accept insta snapshots: `cargo insta review`.

## Crate architecture

Rust workspace with three crates:

- `crates/data-dict/` — core library: YAML parsing, spec validation, lowering to typed model, and semantic checks. All logic lives here.
- `crates/data-dict-cli/` — thin CLI wrapper (`validate-spec` / `validate-meta` / `validate-data`, plus `types parquet`). Keep it thin.
- `crates/data-dict-parquet/` — reads Parquet file schemas and maps column types to data-dict types.

### Validation levels

The three levels and every check code (`S##` / `M##` / `D##`) are defined in `site/validation.md` — the single source of truth. Don't re-document the checks here or in code comments; point to that file. Each level implies the ones before it.

Implementation, one module per level (entry points re-exported at the crate root):

| Level | Module | CLI |
|-------|--------|-----|
| spec (`S##`) | `validate_spec.rs` — structural check against `schema.yaml`, then the semantic `S` checks | `validate-spec` |
| metadata (`M##`) | `validate_meta.rs` | `validate-meta` |
| data (`D##`) | `validate_data.rs` | `validate-data` |

Every level reports through one vocabulary in `problem.rs`: a `Problem` (a `code`, `severity`, `message`, optional `expected`/`column`/`hint`/`span`, and a flattened `ProblemKind` tag covering pre-flight, spec, metadata, and data findings alike) and a `ProblemSet` (one vector of them plus the `SourceContext` for rendering). `serde` derives the JSON wire format directly; there is no separate error type. "Fatal" is not a field — a level pushes its problems and returns early to stop the run, and the meta/data levels descend only while `ProblemSet::has_errors()` is false. `Level`, the `select_tables` helper, and the `compare_dataset`/`read_parquet` driver live in `lib.rs`. Each level's entry point drives its own flow (no central dispatcher).

Test fixtures for the spec rules are in `crates/data-dict/tests/fixtures/{valid,invalid,spec}/`. Each fixture has a `# expected: ...` header documenting the intended outcome. Integration tests mirror the levels: `tests/validate_spec.rs` / `validate_meta.rs` / `validate_data.rs`.

Every test that asserts a diagnostic error or warning must also include a snapshot assertion:

```rust
diagnostic.assert_contains(&["S07", "expected phrase"]);
#[cfg(unix)]
assert_snapshot!(diagnostic);
```

After adding new snapshot assertions, generate them with `cargo insta test -p data-dict --test validate_spec`, inspect the `.snap.new` files to confirm they look right, then accept with `cargo insta accept --workspace`.

### Problem reporting

Two principles guide how problems are surfaced:

- **Full context.** A problem should carry enough context that the user can see at a glance where it comes from — point at the offending span and fade in its enclosing nodes (e.g. the table and column a bad value sits in), so the location is unambiguous without re-reading the file.
- **Report as many problems as possible at once.** Prefer collecting all the problems in a pass over bailing on the first, so the user fixes them together rather than rerunning repeatedly. Not always possible (a problem can block the checks that would follow it), but worth striving for.

### Diagnostic wording

A diagnostic is split across two parts: `expected` is a general statement of the problem, and `message` reports what was found at the offending location. `expected` leads the rendering (the title line beside the code for span-located spec problems; the headline line for the plain-rendered metadata/data problems) and `message` follows it. Prefer this split whenever a general rule can be stated, at every level (`S`/`M`/`D`).

- `expected` is one concise but informative statement, in sentence case, ending with a full stop. State what *must* hold when the cause is clear (e.g. an incorrect type or size: "A range's minimum must be less than or equal to its maximum."); use *can't* when you can't state what was expected.
- `message` (the "found" detail) is a lowercase fragment with no full stop — it names the concrete value or location ("minimum `100` is greater than maximum `10`").
- Diagnostic hints always start with a capital letter.

If a schema change causes `site/examples/` to fail, don't fix them. Instead report them to me so I can fix upstream.

### Restricted columns

A column marked `display: restricted` holds sensitive data (typically PII), and **no value the data held may ever appear in tool output** — exports, rendered HTML, or diagnostics. Enforcement lives in two places:

- `ExportProfile::restrict()` (`export.rs`) strips `sample_values`/`common_values`/`range`/`histogram` from a restricted column's profile, keeping only counts (`missing`, `distinct`). The website renders through this export, so gating here covers it.
- `is_restricted()` (`validate_data.rs`) withholds offending-value samples from the D02/D04/D05/D07 diagnostics (message and `ProblemKind::values` alike). Withholding is per column: a restricted column drops out of every [`ValueRow`](crates/data-dict/src/problem.rs), while the unrestricted columns beside it in a composite key or an assertion keep their values. Each of those kinds carries `redacted`, saying whether anything was withheld.

Any new feature that moves data values toward the user — a new profile statistic, a new export field, a new diagnostic that quotes values — must gate on `display` the same way. Author-declared `examples`/`range` are the author's responsibility (the spec requires fakes); only data-derived values are gated.


## Data format

- Keys in `data-dict.yaml` use snake_case (e.g. `primary_key`, `foreign_key`, `$learn_more`).

## Prose

- Use sentence case for headings.

- If the user asks you to proofread a file, act as an expert proofreader and editor with a deep understanding of clear, engaging, and well-structured writing.

  Work paragraph by paragraph, always starting by making a TODO list that includes individual items for each top-level section.

  Fix spelling, grammar, and other minor problems without asking the user. Label any unclear, confusing, or ambiguous sentences with a FIXME comment.

  Only report what you have changed.


## Code

- Use nanoparquet for reading/writing parquet files (R code).

## Benchmarking

When benchmarking performance work (e.g. the parquet validators):

- Benchmark on a realistic large dataset: a ~10M-row parquet file generated with nanoparquet, not a toy fixture. Use the duckdb row group size, i.e. `num_rows_per_row_group = 122880`
- Always report both speed and peak memory (RSS), as a summary table. Use best-of-N timings (e.g. best of 3).
- Isolate component costs (e.g. decode vs. hashing/dedup) before optimising, so effort goes where the bottleneck actually is.
- Verify correctness in the same harness (known duplicates, including ones spanning row groups) and re-measure before/after; revert changes that regress.
- Prefer real measurements over guesses when weighing an alternative (e.g. a new dependency vs. hand-rolled): measure binary size, dependency count, build time, and runtime.
- **Find hotspots with `/usr/bin/sample`** (macOS, built-in, no sudo — unlike flamegraph's dtrace). Loop the operation so the process runs ~20–30s, `sample <pid> <secs> 1 -file out.txt`, then read the "Sort by top of stack" section (self-time) piped through `c++filt`. Build with `CARGO_PROFILE_RELEASE_DEBUG=2` for symbols. Ignore `__psynch_cvwait` — that's idle rayon workers, not work.
- **Judge changes with `criterion`** (`crates/data-dict-parquet/benches/uniqueness.rs`, `cargo bench`). Use a saved-baseline A/B — `--save-baseline after`, restore the old code, `--baseline after` — not sequential runs, which overstate gains on a noisy machine. Trust the p-values: only keep a change criterion calls significant, and confirm profiler-suggested tweaks actually help (some hurt — e.g. `collect()` vs. a pre-sized `Vec` defeats vectorisation).
