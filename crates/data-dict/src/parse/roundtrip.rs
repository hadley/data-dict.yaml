//! The oracle: every expression the language can emit reads back as itself.
//!
//! This is the claim `site/expression-execution.md#sources` makes, and the
//! reason the readable surface is bounded by what the emitters produce rather
//! than by "R" — a bounded surface is one a test can enumerate.
//!
//! One corpus drives two checks per dialect. For an expression the pipeline
//! carries through unchanged, both are exact:
//!
//! 1. **Idempotence**: emitting the re-read expression reproduces the R text
//!    exactly. If reading lost or added anything, the second emission diverges.
//! 2. **Identity of the tree**: what comes back is the tree that went out.
//!
//! For the handful the pipeline [normalises](NORMALISED) neither can hold — the
//! whole point is that the reading settles on one of two spellings. What is
//! checked there instead is that it **converges**: normalising twice changes
//! nothing that normalising once didn't, so a dictionary rewritten through this
//! path is stable rather than drifting a little further on each pass.

use crate::assert_expr::{AssertExpr, Root, TypedAssertion, check_root, lower, tests::TestEnv};
use crate::emit::{Canonical, Polars, R_BASE, R_DATA_TABLE, R_TIDYVERSE, Target, emit};

/// Every expression the R emitter's own tests cover, plus the constructs they
/// reach only in combination. Each is written in the language.
const CORPUS: &[&str] = &[
    // Literals and operators.
    "qty > 0",
    "qty = 42",
    "qty = 42.0",
    "qty = 0.5",
    "s = 'it''s'",
    "flag = TRUE",
    "flag = FALSE",
    "qty IS NULL",
    "qty IS NOT NULL",
    "qty = INF",
    "qty = NAN",
    "n + 1 > 0",
    "n - 1 - 2 > 0",
    "n - (1 - 2) > 0",
    "(n + 1) * 2 > 0",
    "n / qty > 1",
    "-n > 0",
    "NOT flag",
    "NOT (q3 OR q4)",
    "q3 AND q4",
    "q3 OR q4",
    "qty > 0 AND flag",
    // Membership, ranges, patterns.
    "qty IN (1, 2, 3)",
    "qty NOT IN (1, 2, 3)",
    "s IN ('a', 'b')",
    "qty BETWEEN 0 AND 100",
    "s LIKE 'NZ-%'",
    "s LIKE '%.nz'",
    "s LIKE 'exact'",
    "s NOT LIKE 'NZ-%'",
    "s SIMILAR TO '[a-z]+'",
    "s LIKE 'a%b'",
    "s LIKE 'a_b'",
    "s LIKE 'a.b%c'",
    // Conditionals.
    "CASE WHEN flag THEN qty > 1 ELSE qty > 10 END",
    "CASE WHEN flag THEN qty > 1 END",
    "CASE WHEN flag THEN 1 WHEN q3 THEN 2 ELSE 3 END",
    // Scalar functions.
    "LENGTH(postcode) <= 10",
    "LOWER(s) = 'a'",
    "UPPER(s) = 'A'",
    "TRIM(s) = 'a'",
    "STARTS_WITH(s, 'NZ-')",
    "ENDS_WITH(s, '.nz')",
    "STARTS_WITH(s, LOWER(postcode))",
    "ABS(n) > 0",
    "FLOOR(qty) > 0",
    "CEIL(qty) > 0",
    "ROUND(qty) > 0",
    "ROUND(qty, 2) > 0",
    "MOD(n, 3) = 0",
    "IS_FINITE(qty)",
    "IS_INFINITE(qty)",
    "IS_NAN(qty)",
    // Aggregates.
    "SUM(qty) > 0",
    "AVG(qty) > 0",
    "MIN(qty) > 0",
    "MAX(qty) > 0",
    "COUNT(s) > 0",
    "COUNT_DISTINCT(s) <= 16",
    "ANY(flag)",
    "ALL(flag)",
    "qty <= 2 * MIN(qty)",
    // Time.
    "ts >= NOW() - interval(2, weeks)",
    "ts >= NOW() - interval(n, days)",
    "d >= '2000-01-01'",
    "d + interval(12, hours) < NOW()",
    // Structs.
    "LENGTH(addr.zip) > 0",
    "addr IS NOT NULL",
    // Selections.
    "COLUMNS('q[34]') IS NOT NULL",
    "COLUMNS(*) IS NOT NULL",
    "COLUMNS([q3, q4]) IS NOT NULL",
];

/// What the pipeline deliberately normalises, so the tree that comes back says
/// the same thing in a different shape. Each is a real, understood loss:
///
/// * `LIKE` with a literal pattern has no R spelling of its own. A prefix or
///   suffix becomes `startsWith`/`endsWith`, which is the language's
///   `STARTS_WITH`/`ENDS_WITH`; an exact pattern becomes `==`; anything else
///   becomes a regex match, which is `SIMILAR TO`. All four say the same thing,
///   and none can be told from the form the R was emitted for.
/// * Base R and data.table have no `COLUMNS(...)`, so a selection is expanded to
///   a conjunction before it is emitted. Nothing in the R marks it as having been
///   a selection, and inventing one back would put a rule in the dictionary that
///   the author didn't write.
const R_NORMALISED: &[&str] = &[
    "s LIKE 'NZ-%'",
    "s LIKE '%.nz'",
    "s LIKE 'exact'",
    "s NOT LIKE 'NZ-%'",
    "s LIKE 'a%b'",
    "s LIKE 'a_b'",
    "s LIKE 'a.b%c'",
    "COLUMNS('q[34]') IS NOT NULL",
    "COLUMNS(*) IS NOT NULL",
    "COLUMNS([q3, q4]) IS NOT NULL",
];

/// polars keeps a selection a selection and needs no guards, so only the `LIKE`
/// spellings normalise.
const POLARS_NORMALISED: &[&str] = &[
    "s LIKE 'NZ-%'",
    "s LIKE '%.nz'",
    "s LIKE 'exact'",
    "s NOT LIKE 'NZ-%'",
    "s LIKE 'a%b'",
    "s LIKE 'a_b'",
    "s LIKE 'a.b%c'",
];

fn ir(source: &str) -> TypedAssertion {
    let expr = AssertExpr::parse(source).unwrap_or_else(|e| panic!("parse({source:?}): {e:?}"));
    let findings = check_root(&expr, &TestEnv, Root::Any);
    assert!(findings.is_empty(), "{source:?}: {findings:?}");
    lower(&expr, &TestEnv).unwrap_or_else(|| panic!("lower({source:?})"))
}

/// Read R, then print it in the language, so a failure reads as two expressions
/// rather than two trees.
fn read_to_canonical(code: &str) -> String {
    let parsed = super::r::read(code).unwrap_or_else(|e| panic!("read({code:?}): {e:?}"));
    let findings = check_root(&parsed.expr, &TestEnv, Root::Any);
    assert!(findings.is_empty(), "{code:?} should check: {findings:?}");
    let ir = lower(&parsed.expr, &TestEnv).unwrap_or_else(|| panic!("lower({code:?})"));
    emit(&Canonical, &ir)
        .expect("the language always emits")
        .code
}

#[test]
fn every_r_emission_reads_back_as_itself() {
    round_trip(
        "r",
        &[
            ("R(tidyverse)", &R_TIDYVERSE),
            ("R(base)", &R_BASE),
            ("R(data.table)", &R_DATA_TABLE),
        ],
        R_NORMALISED,
    );
}

/// polars needs almost no guards, so the only normalisation it has is the one
/// every target shares: a literal `LIKE` pattern has no spelling of its own.
#[test]
fn every_polars_emission_reads_back_as_itself() {
    round_trip("python", &[("Python(polars)", &Polars)], POLARS_NORMALISED);
}

fn round_trip(language: &str, targets: &[(&str, &dyn Target)], normalised: &[&str]) {
    let read = crate::parse::resolve(language).expect("a readable language");
    for source in CORPUS {
        let original = ir(source);
        let canonical = emit(&Canonical, &original).expect("emits").code;
        for (name, target) in targets {
            // A target that refuses this construct has nothing to read back.
            let Ok(emitted) = emit(*target, &original) else {
                continue;
            };
            let parsed = read
                .read(&emitted.code)
                .unwrap_or_else(|e| panic!("{name} {source:?} -> {:?}: {e:?}", emitted.code));
            let findings = check_root(&parsed.expr, &TestEnv, Root::Any);
            assert!(
                findings.is_empty(),
                "{name} {source:?} -> {:?} should check: {findings:?}",
                emitted.code
            );
            let reread = lower(&parsed.expr, &TestEnv)
                .unwrap_or_else(|| panic!("{name} {source:?} -> {:?}", emitted.code));

            let second = emit(*target, &reread).expect("emits").code;
            if normalised.contains(source) {
                // Normalising converges: the second pass is a fixed point.
                let again = read
                    .read(&second)
                    .unwrap_or_else(|e| panic!("{name} {second:?}: {e:?}"));
                let again = lower(&again.expr, &TestEnv).expect("lowers");
                assert_eq!(
                    emit(*target, &again).expect("emits").code,
                    second,
                    "{name} keeps changing {source:?}"
                );
            } else {
                // 1. Emitting again reproduces the code exactly.
                assert_eq!(
                    second, emitted.code,
                    "{name} is not idempotent on {source:?}"
                );
                // 2. And the tree is the one it started as.
                assert_eq!(
                    emit(&Canonical, &reread).expect("emits").code,
                    canonical,
                    "{name} changed the meaning of {source:?} via {:?}",
                    emitted.code
                );
            }
        }
    }
}

/// The normalisations are real, and each one lands where it is documented to.
#[test]
fn a_normalised_expression_still_says_the_same_thing() {
    assert_eq!(
        read_to_canonical("startsWith(s, \"NZ-\")"),
        "STARTS_WITH(s, 'NZ-')"
    );
    assert_eq!(
        read_to_canonical("endsWith(s, \".nz\")"),
        "ENDS_WITH(s, '.nz')"
    );
    assert_eq!(read_to_canonical("s == \"exact\""), "s = 'exact'");
    // Base R expands a selection to the conjunction it stands for, and that is
    // what comes back — a conjunction, not a selection.
    assert_eq!(
        read_to_canonical("!is.na(q3) & !is.na(q4)"),
        "q3 IS NOT NULL AND q4 IS NOT NULL"
    );
    // The tidyverse keeps it a selection, so that one does round-trip.
    assert_eq!(
        read_to_canonical("if_all(matches(\"q[34]\"), \\(x) !is.na(x))"),
        "COLUMNS('q[34]') IS NOT NULL"
    );
}
