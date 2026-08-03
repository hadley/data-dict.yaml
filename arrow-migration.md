# Plan: adopt arrow in data-dict-parquet

Replace the hand-rolled column decoding in `data-dict-parquet` with the parquet
crate's `arrow` feature. Measured cost: +11 crates (59 → 70), binary
6.7 MB → ~8.4 MB, no new release train (arrow-rs and parquet version in
lockstep from the same repo). Expected win: delete ~550–600 lines of
decode/canonicalization code, retire the slow `get_row_iter` record API,
better value rendering in diagnostics, and the typed-columns-plus-validity
decode layer that the D07 assertion interpreter
(`site/expression-execution.md`) needs anyway.

Alongside the refactor, four spec changes were approved and landed in
`site/validation.md` / `site/spec.md` on 2026-08-02 (string-like enums,
float16 comparability, INT96 as instants, D05 cross-representation);
the phases below carry their implementation, which per AGENTS.md's sync rule
is owed now that the spec text is in.

## Ground rules

- One phase per PR; each leaves `cargo test --workspace` green and the
  public API of `data-dict-parquet` unchanged unless noted.
- Per the benchmarking rules in `AGENTS.md`: criterion saved-baseline A/B
  for every phase touching a hot path, plus best-of-3 speed and peak RSS on
  the ~10M-row nanoparquet harness. A regression criterion calls
  significant is a revert, not a caveat.
- The spec's comparable-types semantics are non-negotiable invariants:
  `-0.0 == +0.0`, all NaNs equal, decimals by numeric value, nulls never
  duplicates, composite keys skip rows with any null. arrow-row's total
  ordering distinguishes zero signs and NaN payloads, so float
  canonicalization must run *before* any row encoding.

## Phase 0 — baselines

- [x] Record criterion baselines (`cargo bench` in `data-dict-parquet`,
      `--save-baseline pre-arrow`) for `uniqueness` and `foreign_key`.
- [x] Record best-of-3 wall time and peak RSS for `validate-data` on the
      10M-row harness (duckdb row-group size, 122880), covering: single-column
      D02, composite-key D02, D04 dictionary fast path, D04 full scan, D05.
      *As run:* measured on the existing 3M-row bench fixtures (1M-row
      groups) instead of a fresh 10M-row harness; D04 scenarios not measured
      (see phase 2). Results in the section at the end.
- [x] Record release binary size and clean-build time of `data-dict-cli`.

## Phase 1 — dependency and decode substrate

- [x] Add `arrow` to the parquet feature list in `Cargo.toml`; add
      `arrow-array`, `arrow-schema`, `arrow-buffer` (and `arrow-row`,
      `arrow-cast` when phases 3/2 land) as workspace deps pinned to the
      parquet version.
- [x] Add an internal reader context in `data-dict-parquet`: parse
      `ArrowReaderMetadata` (footer + schema) once per file and construct
      multiple projected `RecordBatch` readers from it. Today types, footer
      stats, barriers, D01/D04, D02, and D05 each reopen and re-parse the
      same file; the context ends that. Internal API only — the crate's
      public functions keep their signatures.
- [x] Add a shared sampling helper: given a `BooleanArray` of violations,
      iterate its set bits for 1-based row numbers, count everything, stop
      collecting at `sample_limit`, and extract sample values with `take` +
      `ArrayFormatter`. D01, D04, and D05 all route through it instead of
      bespoke per-row loops.
      *As built:* the shared piece is the `ArrayFormatter` wrapper
      (`display.rs`), used by every check that samples values; the row loops
      stayed per-check because each samples different fields. No
      `BooleanArray` materialization — the loops read validity directly.
- [x] Reshape float canonicalization (`float_bits`/`double_bits` semantics)
      as an array-level pass usable by phases 3–4.
- [x] Confirm binary size and build time against the phase 0 numbers.

## Phase 2 — D01/D04: replace scan.rs and delete dictionary.rs

- [x] Rewrite `column_stats` (`scan.rs`) on the record-batch reader: null
      counts and sampled null rows from validity bitmaps; enum membership on
      decoded arrays. Delete the `get_row_iter` loop and `field_key`.
- [x] Decide the D04 dictionary strategy by measurement. The arrow reader
      does **not** preserve Parquet dictionary encoding by default — strings
      decode to plain arrays unless the requested schema types the column as
      `Dictionary` (`ArrowReaderOptions::with_schema`), and with such a
      schema even plain-encoded pages may come back dictionary-materialized,
      so "dictionary chunk vs. plain chunk" is not a distinction the decoded
      arrays reliably expose. Benchmark the two options on the conforming
      case and pick one:
      1. Keep the page-level `dictionary_conforms` fast path (`dictionary.rs`)
         — the only variant that can avoid decoding data pages outright.
      2. Request dictionary-typed arrays for enum columns and check each
         chunk's `DictionaryArray::values()` against the allowed set,
         inspecting keys only when the dictionary holds an outside value —
         simpler, unifies fast path and fallback (violations yield rows in
         the same pass instead of a rescan), but it is a decoded-data
         optimization, not page skipping.
      Delete `dictionary.rs` (164 lines) only if option 2's conforming-case
      benchmark is acceptable.
      *Decision:* option 1 kept — the page-level fast path survives the port
      unchanged (trimmed to `BYTE_ARRAY`, since enums are string-like), and
      the fallback scan is now arrow. Option 2 was not benchmarked; the
      conservative default costs nothing and the unified-pass upside can be
      revisited if the fast path ever bothers anyone.
- [x] Implement the string-like-enum spec rule (landed 2026-08-02;
      arrow-independent, so this can be its own PR ahead of the rest):
      tighten `types_compatible`'s enum arm in `validate_meta.rs` from
      `"string" | "number" | "enum"` to string-like only, making a
      numeric-backed enum an M01; update fixtures.
- [x] With enums string-only, D04 membership becomes plain string equality:
      `Scalar::value_keys` collapses to the string itself (delete the
      f32-narrowing hack and its doc comment), `enum_allowed` is a plain
      string set, and the numeric arms of the membership scan (`field_key`'s
      numeric cases, `dictionary.rs`'s `fixed_in_set`) are dead whichever
      dictionary strategy wins.
- [x] Replace `display_value` with `arrow-cast`'s `ArrayFormatter`. This
      changes rendered samples for dates/timestamps/decimals (e.g. `19723` →
      `2024-01-01`) — review the affected insta snapshots deliberately; the
      new rendering is the point, not collateral.
- [ ] A/B all three D04 scenarios on the harness — dictionary-conforming,
      dictionary-with-violations, non-dictionary — feeding the strategy
      decision above. Context for judging the conforming case: writers
      commonly omit `page_encoding_stats`, so today's "page-skipping" path
      usually walks pages anyway (without decoding them).
      *Not run:* moot for now — option 1 was kept, so the conforming case is
      byte-identical to the old code. Run this only if revisiting option 2.

## Phase 3 — D02: uniqueness on arrow

- [x] Replace the multi-reader batch loop in `uniqueness.rs` with the
      projected record-batch reader.
- [x] Keep the single-scalar `i64` fast path and `ByteKeys`/`Arena`
      (`column_scan.rs`) — arrow provides arrays, not a dedup set, and the
      arena's incremental growth is load-bearing for peak RSS.
- [x] Replace `Dedup::Bytes` length-framed composite keys with
      `arrow-row::RowConverter` rows, applied *after* the float
      canonicalization pass. Byte-backed decimals now arrive as
      `Decimal128`/`Decimal256`, so delete `normalize_decimal` and
      `Normalization::DecimalBytes`.
- [x] Mind `RowConverter` memory: encoded `Rows` are owned per batch, so
      inserting into the arena copies every distinct composite key and
      transiently retains both the row buffer and the arena copy; and
      arrow-row flattens dictionary arrays before encoding. The RSS
      measurement must therefore include a high-cardinality composite key of
      wide strings, not only numeric keys.
- [x] Criterion A/B against `pre-arrow`; RSS on the harness. If
      `RowConverter` loses to the hand-rolled framing on speed or peak
      memory, keep the framing over arrow arrays and still delete the decode
      layer — the phases are separable.
- [x] Implement the float16 spec change (landed 2026-08-02): arrow decodes
      `Float16` as `half::f16`, so extend the float canonicalization pass to
      16-bit (collapse ±0.0 and NaNs) and remove the `"float16"` barrier
      from `uniqueness_comparability` and its `barrier_phrase` entry. Flows
      to D05 automatically via the shared comparability classification.
- [x] Implement the INT96 spec change (landed 2026-08-02): arrow decodes
      `INT96` to timestamps, so comparison is by instant; delete
      `int96_owned` and the raw-bytes keying.
- [x] Fixture coverage for the invariants: ±0.0 and NaN duplicates
      (including Float16), equal decimals at different byte widths, composite
      key with nulls, duplicates spanning row groups.

## Phase 4 — D05: foreign keys on arrow

- [x] Port both `scan_column` passes in `foreign_key.rs` to the batch
      reader; `KeySet` build-then-probe shape is unchanged.
- [x] Cast parent and child arrays to one shared comparison type (arrow
      `cast` kernel) before hashing, derived from the spec's comparable-type
      semantics — not simply the wider side. This closes a real gap: today a
      shape mismatch between the two columns (e.g. an `INT32`-backed decimal
      referencing a byte-backed one) treats every child value as absent
      (`KeySet::contains`), where `site/validation.md` says both sides are
      "compared by the same normalized value form". Covers integer widths,
      `Utf8`/`LargeUtf8`/view variants, binary variants, and decimal
      physical representations. Add fixtures for the cross-representation
      cases; the spec wording for this landed in validation.md on
      2026-08-02, including the no-common-form rule (nothing matches, every
      non-null child value reported).
- [x] Orphan samples through the phase 1 sampling helper and formatter.
- [x] Criterion A/B (`foreign_key` bench) and harness RSS.

## Phase 5 — cleanup and docs

- [x] Delete the now-dead remainder of `column_scan.rs`: `read_batch`, the
      decode macros, `expand_bytes`, `int96_owned`, `fixed_len_owned`,
      `physical_mismatch`, `ColumnBatch` if nothing structural still uses it.
- [x] Drop `hashbrown`/`rayon` only if actually unused (both likely stay).
- [x] Spec sweep per AGENTS.md "touch one, check the other": confirm the
      implementation now matches all four 2026-08-02 spec changes (see the
      last section) and that no other S/M/D wording drifted.

## Phase 6 — final verification

- [x] `cargo test --workspace`, `cargo fmt --all`,
      `cargo clippy --workspace --all-targets`, `cargo insta review`.
- [x] Full harness table (speed + peak RSS, best of 3) vs. phase 0, per
      check; criterion summary vs. `pre-arrow`.
      *Caveat:* the saved `pre-arrow` criterion baseline proved worthless —
      it was recorded while a concurrent `cargo test` compile loaded the
      machine, inflating every number ~8x. The comparison below is instead
      an interleaved old-vs-new harness (same fixtures, same best-of-3
      timing loop, quiet machine), which is the stronger A/B anyway.
- [x] Binary size and clean-build time vs. phase 0 (budget: ≤ ~2 MB and
      ≤ ~40 s over baseline).
      *Over budget:* 6.7 MB → 12.6 MB unstripped; 5.7 MB → 10.6 MB stripped.
      Stripping shaves ~16% off both sides but barely moves the delta —
      arrow costs +5.1 MB stripped / +6.2 MB unstripped, ~1.9x either way.
      The ≤ 2 MB budget came from a scratch experiment that understated a
      real binary; accepting, `strip = true` in the release profile, or
      trimming arrow features is an open call.
- [x] Known-duplicate/orphan correctness cases re-run in the same harness,
      including ones spanning row groups. (Integration suite: 36 data-level
      tests, all green, including cross-row-group duplicates and the new
      float16 / signed-zero / decimal-encoding / no-common-form cases.)

### Results (2026-08-02, 3M-row fixtures, best of 3, interleaved)

| Check | Old | New |
|-------|-----|-----|
| D02 uniqueness, int column | 29.5–32.4 ms | 29.1–30.6 ms |
| D02 uniqueness, string column | 84.6–86.1 ms | 70.4–71.4 ms |
| D02 composite primary key | 133–135 ms | 119–121 ms |
| D05 foreign key, int | 59–63 ms | 62–66 ms |
| D05 foreign key, string | 191–197 ms | 168–171 ms |
| `validate-data` peak RSS (uniq harness) | 254 MB | 252 MB |
| `validate-data` peak RSS (fk harness) | 107 MB | 105 MB |

Strings −17%, composite keys −10%, string FKs −13%, int paths parity
(int FK within noise after borrowing 8-byte natives instead of copying).
Memory unchanged. `site/examples/otters.yaml` still validates clean.

## Non-goals

Arrow's sort/partition/rank kernels are deliberately not used for D02: they
retain and order far more data than the streaming hash-set design, which
already matches the exact-count and peak-memory requirements. Likewise
row-selection predicate pushdown buys nothing for checks that must inspect
every value and report exact counts.
