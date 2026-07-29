# Plan next

Work worth doing that isn't blocking anything.

## Unify enum membership and profiling on typed values

`dictionary.rs` (the D04 enum fast path) and `profile.rs` both read dictionary
pages, and they still decode them separately. The shared byte walk now lives in
`page.rs`, but the decoded values themselves don't line up, because the two
sides compare in different vocabularies:

- **D04** tests against a `HashSet<String>` of canonical renderings from
  `Scalar::value_keys()`, which deliberately emits *two* keys for a float — the
  `f64` and the `f32` spelling — so one YAML `1.1` matches both a `DOUBLE` and a
  `FLOAT` column.
- **Profiling** produces typed `Value`s (`value.rs`), where a float is one
  finite `f64` and comparison never goes through a string.

So `dictionary_in_set` can't call `decode_dictionary`, and each keeps its own
per-physical-type match: `fixed_in_set::<N>` beside `decode_fixed::<N>`.

The end state is probably that `dictionary.rs` becomes pure mechanism — read a
dictionary page, hand back `Vec<Value>` — with D04's policy (is every value in
the allowed set?) moving next to the rest of D04 in `scan.rs`, and the allowed
set becoming typed `Value`s rather than strings. That would also drop the
stringify-to-compare step from `scan.rs`'s `field_key`.

Worth its own review rather than a drive-by, because it changes how D04 decides
equality. The float double-key trick is load-bearing: `float_enum_values_compare_at_column_width`
and `large_integer_enum_values_compare_exactly` in `crates/data-dict/tests/validate_data.rs`
are the tests that pin the current behaviour, and any typed replacement has to
keep them passing without widening or narrowing what counts as a match.

## Reconsider the histogram's second pass if files without statistics turn up

`profile_column` falls back to a second scan when the footer has no exact
min/max, because bin edges have to be known before any value can be binned.
Nearly every writer records statistics, so this is the rare path today.

If that stops being true — or if the second pass ever shows up as a real cost on
a benchmark-sized file — the fix is a quantile sketch (KLL or t-digest), which
gets the range and the distribution in one pass. Both keep exact min/max, so the
20-equal-width-bin output could survive unchanged, with the counts becoming
approximate. Avoid the log-bucket family (DDSketch, Prometheus native
histograms): their guarantee is *relative* error, which is meaningless for
temporal columns, where values are offsets from an arbitrary epoch. At 1%
relative error a nanosecond timestamp lands every value within ~200 days in one
bucket, collapsing a year of data into two.
