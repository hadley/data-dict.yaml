//! Reading R.
//!
//! The surface is exactly what the three [`R` targets](crate::emit) emit —
//! base R, dplyr/stringr, and data.table — plus the spellings an author would
//! naturally write for the same thing. That bound is what makes the surface a
//! finite, testable list rather than "R", and it makes the round trip a property
//! that can be checked: every expression the language can emit as R reads back
//! as itself. See `site/expression-execution.md#sources`.
//!
//! One reader takes all three dialects at once. It can, because they use
//! disjoint names wherever they differ — `nchar` against `str_length`, `ifelse`
//! against `if_else` against `fifelse` — so no R text's meaning depends on being
//! told which dialect it is. That is why a source is named by family alone where
//! a target is named `family(dialect)`.
//!
//! Three stages, in order:
//!
//! 1. [`parse`] scans the text into a faithful [R tree](ast), keeping the things
//!    the language has no equivalent of — named arguments, `~`, `\(x)`, `$`, `[`.
//! 2. [`fold`] recognises the idioms that stand for exactly one of the
//!    language's constructs, including the guards an emitter adds.
//! 3. [`map`] reads what is left a node at a time, noting each place R's meaning
//!    and the reading's differ.

mod ast;
mod fold;
mod map;
mod parse;

use crate::assert_expr::{AssertExpr, ParseError};
use crate::parse::{Notes, Parsed};

pub fn read(source: &str) -> Result<Parsed, ParseError> {
    let tree = parse::parse(source)?;
    let mut notes = Notes::default();
    let root = map::Mapper::new(&mut notes).expr(&tree)?;
    Ok(Parsed {
        expr: AssertExpr { root },
        notes: notes.into_vec(),
    })
}

#[cfg(test)]
mod tests {
    use crate::assert_expr::{Root, check_root, lower, tests::TestEnv};
    use crate::emit::{Canonical, emit};

    /// Read R and print it in the language, so a test reads as two expressions.
    #[track_caller]
    fn dd(code: &str) -> String {
        let parsed = super::read(code).unwrap_or_else(|e| panic!("read({code:?}): {}", e.message));
        let findings = check_root(&parsed.expr, &TestEnv, Root::Any);
        assert!(findings.is_empty(), "{code:?}: {findings:?}");
        let ir = lower(&parsed.expr, &TestEnv).unwrap_or_else(|| panic!("lower({code:?})"));
        emit(&Canonical, &ir).expect("emits").code
    }

    #[track_caller]
    fn refused(code: &str) -> String {
        match super::read(code) {
            Err(e) => e.message,
            Ok(parsed) => panic!("{code:?} should be refused, read as {:?}", parsed.expr.root),
        }
    }

    fn notes(code: &str) -> Vec<&'static str> {
        super::read(code).expect("reads").notes
    }

    // -- precedence, which must invert the emitter's exactly ---------------

    #[test]
    fn an_infix_operator_binds_tighter_than_multiplication() {
        // R's `%…%` sits above `*` and `/`, unlike anything in SQL. Getting
        // this wrong would silently regroup every emitted `MOD`.
        assert_eq!(dd("n %% 3 * 2 == 0"), "MOD(n, 3) * 2 = 0");
        assert_eq!(dd("2 * n %% 3 == 0"), "2 * MOD(n, 3) = 0");
    }

    #[test]
    fn logic_binds_looser_than_comparison_and_not() {
        assert_eq!(dd("qty > 0 & flag"), "qty > 0 AND flag");
        assert_eq!(dd("q3 | q4 & flag"), "q3 OR q4 AND flag");
        assert_eq!(dd("!flag & q3"), "NOT flag AND q3");
        assert_eq!(dd("!(flag & q3)"), "NOT (flag AND q3)");
    }

    #[test]
    fn arithmetic_associates_to_the_left() {
        assert_eq!(dd("n - 1 - 2 > 0"), "n - 1 - 2 > 0");
        assert_eq!(dd("n - (1 - 2) > 0"), "n - (1 - 2) > 0");
        assert_eq!(dd("(n + 1) * 2 > 0"), "(n + 1) * 2 > 0");
        assert_eq!(dd("n + 1 * 2 > 0"), "n + 1 * 2 > 0");
    }

    // -- spellings the emitter never produces ------------------------------

    #[test]
    fn all_three_dialects_read_without_being_named() {
        // The dialects use disjoint names, so one reader takes them all — which
        // is why a source is a family and not a `family(dialect)`.
        for code in ["nchar(postcode)", "str_length(postcode)"] {
            assert_eq!(dd(&format!("{code} <= 10")), "LENGTH(postcode) <= 10");
        }
        for code in ["tolower(s)", "str_to_lower(s)"] {
            assert_eq!(dd(&format!("{code} == \"a\"")), "LOWER(s) = 'a'");
        }
        for code in [
            "ifelse(flag, 1, 2)",
            "if_else(flag, 1, 2)",
            "fifelse(flag, 1, 2)",
        ] {
            assert_eq!(
                dd(&format!("{code} > 0")),
                "CASE WHEN flag THEN 1 ELSE 2 END > 0"
            );
        }
    }

    #[test]
    fn a_number_is_an_integer_unless_it_says_otherwise() {
        // R writes every number as a double, so what decides the reading is
        // whether the source said integer.
        assert_eq!(dd("qty == 42L"), "qty = 42");
        assert_eq!(dd("qty == 42"), "qty = 42");
        assert_eq!(dd("qty == 42.0"), "qty = 42.0");
        assert_eq!(dd("qty == 0.5"), "qty = 0.5");
        assert_eq!(dd("qty == 1e3"), "qty = 1000.0");
    }

    #[test]
    fn r_constants_read_as_the_languages_own() {
        assert_eq!(dd("flag == TRUE"), "flag = TRUE");
        assert_eq!(dd("flag == T"), "flag = TRUE");
        assert_eq!(dd("qty == Inf"), "qty = INF");
        assert_eq!(dd("qty == -Inf"), "qty = -INF");
        assert_eq!(dd("qty == NaN"), "qty = NAN");
        // R's `NA` is the missing value the language calls `NULL`.
        assert_eq!(dd("is.na(qty)"), "qty IS NULL");
    }

    #[test]
    fn a_string_uses_rs_escapes_not_the_languages() {
        assert_eq!(dd("s == \"it's\""), "s = 'it''s'");
        assert_eq!(dd("s == 'single'"), "s = 'single'");
        assert_eq!(dd(r#"s == "a\"b""#), "s = 'a\"b'");
    }

    #[test]
    fn a_field_is_reached_with_a_dollar() {
        // A dot is part of an R name, so `$` is the only field access.
        assert_eq!(dd("nchar(addr$zip) > 0"), "LENGTH(addr.zip) > 0");
    }

    #[test]
    fn the_row_count_has_three_spellings() {
        assert_eq!(dd("n() > 0"), "ROW_COUNT() > 0");
        assert_eq!(dd(".N > 0"), "ROW_COUNT() > 0");
    }

    #[test]
    fn a_multi_branch_conditional_collapses_into_one_case() {
        // Base R spells a multi-branch `CASE` as nested `ifelse`, so the
        // nesting has to be absorbed rather than read as a nested `CASE`.
        assert_eq!(
            dd("ifelse(flag, 1, ifelse(q3, 2, 3)) > 0"),
            "CASE WHEN flag THEN 1 WHEN q3 THEN 2 ELSE 3 END > 0"
        );
        // A trailing `NA` is the `ELSE`-less form.
        assert_eq!(
            dd("ifelse(flag, 1, NA) > 0"),
            "CASE WHEN flag THEN 1 END > 0"
        );
        assert_eq!(
            dd("case_when(flag ~ 1, q3 ~ 2, .default = 3) > 0"),
            "CASE WHEN flag THEN 1 WHEN q3 THEN 2 ELSE 3 END > 0"
        );
        assert_eq!(
            dd("fcase(flag, 1, q3, 2, default = 3) > 0"),
            "CASE WHEN flag THEN 1 WHEN q3 THEN 2 ELSE 3 END > 0"
        );
        // The pre-`.default` dplyr idiom.
        assert_eq!(
            dd("case_when(flag ~ 1, TRUE ~ 2) > 0"),
            "CASE WHEN flag THEN 1 ELSE 2 END > 0"
        );
    }

    #[test]
    fn an_unanchored_pattern_grows_the_wildcards_that_say_so() {
        // `SIMILAR TO` matches the whole string; `grepl` matches anywhere.
        assert_eq!(dd("grepl(\"^(?:[a-z]+)$\", s)"), "s SIMILAR TO '[a-z]+'");
        assert_eq!(dd("grepl(\"abc\", s)"), "s SIMILAR TO '.*(?:abc).*'");
        // The group matters: a bare `.*a|b.*` would regroup around the `|`.
        assert_eq!(dd("grepl(\"a|b\", s)"), "s SIMILAR TO '.*(?:a|b).*'");
        // stringr takes its arguments the other way round.
        assert_eq!(dd("str_detect(s, \"abc\")"), "s SIMILAR TO '.*(?:abc).*'");
    }

    // -- folds, each with a near miss that must not fire -------------------

    #[test]
    fn the_membership_guard_folds_but_a_bare_test_does_not() {
        // The emitter's guarded form is exactly `IN`...
        assert_eq!(dd("is.na(qty) | (qty %in% c(1, 2))"), "qty IN (1, 2)");
        assert_eq!(dd("is.na(qty) | !(qty %in% c(1, 2))"), "qty NOT IN (1, 2)");
        // ...so it earns no note of its own, where the bare test does.
        assert!(
            !notes("is.na(qty) | (qty %in% c(1, 2))")
                .iter()
                .any(|n| n.contains("%in%"))
        );
        assert!(notes("qty %in% c(1, 2)").iter().any(|n| n.contains("%in%")));
        // A guard over a *different* column is an ordinary `OR`.
        assert_eq!(
            dd("is.na(n) | (qty %in% c(1, 2))"),
            "n IS NULL OR qty IN (1, 2)"
        );
    }

    #[test]
    fn the_null_guard_is_stripped_only_when_it_guards_the_body() {
        assert_eq!(
            dd("ifelse(is.na(s), NA, grepl(\"^(?:a)$\", s))"),
            "s SIMILAR TO 'a'"
        );
        // The guarded operand doesn't appear in the body, so this is an
        // ordinary conditional that happens to test for missingness.
        assert_eq!(
            dd("ifelse(is.na(n), NA, flag)"),
            "CASE WHEN n IS NULL THEN NULL ELSE flag END"
        );
    }

    #[test]
    fn the_kind_tests_fold_back_out_of_their_guards() {
        assert_eq!(
            dd("if_else(is.na(qty) & !is.nan(qty), NA, is.nan(qty))"),
            "IS_NAN(qty)"
        );
        assert_eq!(
            dd("ifelse(is.na(qty) & !is.nan(qty), NA, is.finite(qty))"),
            "IS_FINITE(qty)"
        );
    }

    #[test]
    fn count_folds_out_of_its_sum_and_a_plain_sum_stays_one() {
        assert_eq!(dd("sum(!is.na(s)) > 0"), "COUNT(s) > 0");
        assert_eq!(dd("sum(qty, na.rm = TRUE) > 0"), "SUM(qty) > 0");
        // Base R's distinct count, the one idiom that uses `[`.
        assert_eq!(
            dd("length(unique(s[!is.na(s)])) <= 16"),
            "COUNT_DISTINCT(s) <= 16"
        );
        // A subset of a *different* column isn't the idiom, so nothing folds
        // and the bare `length()` — which counts nulls too — is refused.
        assert!(refused("length(unique(s[!is.na(postcode)]))").contains("`length()`"));
    }

    #[test]
    fn between_folds_from_both_spellings_but_not_from_two_columns() {
        assert_eq!(dd("qty >= 0 & qty <= 100"), "qty BETWEEN 0 AND 100");
        assert_eq!(dd("between(qty, 0, 100)"), "qty BETWEEN 0 AND 100");
        // Two different subjects is a conjunction, not a range.
        assert_eq!(dd("qty >= 0 & n <= 100"), "qty >= 0 AND n <= 100");
    }

    #[test]
    fn an_aggregate_without_na_rm_says_so() {
        assert!(
            notes("sum(qty) > 0")
                .iter()
                .any(|n| n.contains("without `na.rm"))
        );
        // An explicit `FALSE` is what leaving it out means, so it reads the
        // same way rather than being refused.
        assert!(
            notes("sum(qty, na.rm = FALSE) > 0")
                .iter()
                .any(|n| n.contains("without `na.rm"))
        );
        assert!(
            !notes("sum(qty, na.rm = TRUE) > 0")
                .iter()
                .any(|n| n.contains("without `na.rm"))
        );
    }

    // -- refusals ----------------------------------------------------------

    #[test]
    fn a_construct_with_no_equivalent_names_itself() {
        assert!(refused("sapply(qty, is.na)").contains("`sapply()`"));
        assert!(refused("qty %/% 2 == 0").contains("`%/%`"));
        assert!(refused("qty ^ 2 > 0").contains("`^`"));
        assert!(refused("flag && q3").contains("&&"));
        assert!(refused("flag || q3").contains("||"));
        assert!(refused("if_any(everything(), \\(x) !is.na(x))").contains("`if_any`"));
        assert!(refused("qty %in% n").contains("written-out list"));
        assert!(refused("sum(qty, na.rm = flag) > 0").contains("`na.rm"));
        assert!(refused("qty <- 1").contains("assignment"));
        assert!(refused("as.POSIXct(\"2024-01-01\", tz = \"EST\")").contains("UTC"));
        assert!(refused("0x1F > 0").contains("hexadecimal"));
        assert!(refused("NULL").contains("absent value"));
    }

    #[test]
    fn a_refusal_reads_differently_from_a_syntax_error() {
        // Both are `ParseError`s, but one says the rule has to be rewritten and
        // the other that the text is malformed.
        assert!(refused("sapply(x, f)").contains("cannot be translated"));
        assert!(refused("nchar(postcode").contains("expected"));
    }

    #[test]
    fn a_selection_needs_a_selector_the_language_has() {
        assert_eq!(
            dd("if_all(everything(), \\(x) !is.na(x))"),
            "COLUMNS(*) IS NOT NULL"
        );
        assert_eq!(
            dd("if_all(c(q3, q4), \\(x) !is.na(x))"),
            "COLUMNS([q3, q4]) IS NOT NULL"
        );
        // `function(x)` is the same thing spelled out.
        assert_eq!(
            dd("if_all(everything(), function(x) !is.na(x))"),
            "COLUMNS(*) IS NOT NULL"
        );
        assert!(refused("if_all(starts_with(\"q\"), \\(x) !is.na(x))").contains("selection"));
    }
}
