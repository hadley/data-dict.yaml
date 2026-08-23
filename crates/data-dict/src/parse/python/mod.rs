//! Reading Python, in the polars expression style.
//!
//! The surface is what the [`Python(polars)` target](crate::emit) emits, plus
//! the spellings an author would naturally write for the same thing — the same
//! bound, and for the same reason, as [the R reader](super::r).
//!
//! Shorter than the R reader, because polars needs almost no guards. `is_null`,
//! `is_in`, `is_nan` and the string methods all propagate a null the way the
//! language does, so most of what an emitter writes reads straight back. Only
//! two shapes fold: the `drop_nulls()` before an `n_unique()`, and the cast that
//! promotes a date before a duration is added to it.
//!
//! A column is `pl.col("name")`, which is the convention
//! `site/expression-execution.md` fixes for this family.

mod ast;
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

    /// Read polars and print it in the language, so a test reads as two
    /// expressions rather than two trees.
    #[track_caller]
    fn dd(code: &str) -> String {
        let parsed = super::read(code)
            .unwrap_or_else(|e| panic!("read({code:?}): {}", crate::parse::classify(&e).0));
        let findings = check_root(&parsed.expr, &TestEnv, Root::Any);
        assert!(findings.is_empty(), "{code:?}: {findings:?}");
        let ir = lower(&parsed.expr, &TestEnv).unwrap_or_else(|| panic!("lower({code:?})"));
        emit(&Canonical, &ir).expect("emits").code
    }

    #[track_caller]
    fn refused(code: &str) -> String {
        match super::read(code) {
            Err(e) => crate::parse::classify(&e).0.to_string(),
            Ok(parsed) => panic!("{code:?} should be refused, read as {:?}", parsed.expr.root),
        }
    }

    fn notes(code: &str) -> Vec<&'static str> {
        super::read(code).expect("reads").notes
    }

    /// Python's `&` and `|` bind tighter than comparison, so the parentheses an
    /// emitter puts in are load-bearing and have to be read back the same way.
    #[test]
    fn logic_binds_tighter_than_comparison() {
        assert_eq!(
            dd(r#"(pl.col("qty") > pl.lit(0)) & pl.col("flag")"#),
            "qty > 0 AND flag"
        );
        // Without them Python groups the `&` first, which is a different
        // expression — and one this reader reproduces faithfully rather than
        // silently correcting.
        assert_eq!(
            dd(r#"pl.col("q3") & pl.col("q4") | pl.col("flag")"#),
            "q3 AND q4 OR flag"
        );
    }

    /// Python chains comparisons and the language does not, so a chain is
    /// refused rather than read as a left-nested pair.
    #[test]
    fn a_chained_comparison_is_refused() {
        assert!(refused(r#"pl.lit(0) < pl.col("qty") < pl.lit(10)"#).contains("chained"));
    }

    #[test]
    fn columns_literals_and_operators_read_back() {
        assert_eq!(dd(r#"pl.col("qty") > pl.lit(0)"#), "qty > 0");
        // A bare Python literal beside a column means the same thing.
        assert_eq!(dd(r#"pl.col("qty") > 0"#), "qty > 0");
        assert_eq!(dd(r#"pl.col("qty") == pl.lit(42.0)"#), "qty = 42.0");
        assert_eq!(dd(r#"pl.col("flag") == pl.lit(True)"#), "flag = TRUE");
        assert_eq!(dd(r#"pl.col("qty") == float("nan")"#), "qty = NAN");
        assert_eq!(dd(r#"pl.col("qty") == float("inf")"#), "qty = INF");
        assert_eq!(dd(r#"~pl.col("flag")"#), "NOT flag");
        assert_eq!(
            dd(r#"pl.col("n") % pl.lit(3) == pl.lit(0)"#),
            "MOD(n, 3) = 0"
        );
        assert_eq!(
            dd(r#"pl.col("addr").struct.field("zip").is_not_null()"#),
            "addr.zip IS NOT NULL"
        );
    }

    /// `~` in front of one of these is the language's own negation, not a
    /// `NOT` wrapped around it — and it is how the target emits them.
    #[test]
    fn a_negated_predicate_folds_into_itself() {
        assert_eq!(dd(r#"~pl.col("qty").is_null()"#), "qty IS NOT NULL");
        assert_eq!(dd(r#"~pl.col("qty").is_in([1, 2])"#), "qty NOT IN (1, 2)");
        assert_eq!(
            dd(r#"~pl.col("qty").is_between(0, 100)"#),
            "qty NOT BETWEEN 0 AND 100"
        );
        assert_eq!(
            dd(r#"~pl.col("s").str.starts_with("NZ-")"#),
            "NOT STARTS_WITH(s, 'NZ-')"
        );
    }

    #[test]
    fn the_two_guards_fold_and_their_absence_is_noted() {
        // Dropping the nulls first is what `COUNT_DISTINCT` means...
        assert_eq!(
            dd(r#"pl.col("s").drop_nulls().n_unique() <= 16"#),
            "COUNT_DISTINCT(s) <= 16"
        );
        assert!(
            !notes(r#"pl.col("s").drop_nulls().n_unique()"#)
                .iter()
                .any(|n| n.contains("n_unique"))
        );
        // ...so a bare `n_unique`, which counts a null as a value, is noted.
        assert_eq!(
            dd(r#"pl.col("s").n_unique() <= 16"#),
            "COUNT_DISTINCT(s) <= 16"
        );
        assert!(
            notes(r#"pl.col("s").n_unique()"#)
                .iter()
                .any(|n| n.contains("n_unique"))
        );
        // The cast that promotes a date is the emitter's, and the language
        // promotes on its own.
        assert_eq!(
            dd(r#"pl.col("d").cast(pl.Datetime("us")) + pl.duration(hours=12)"#),
            "d + interval(12, hours)"
        );
    }

    #[test]
    fn a_conditional_reads_from_the_outside_in() {
        assert_eq!(
            dd(r#"pl.when(pl.col("flag")).then(1).otherwise(2) > 0"#),
            "CASE WHEN flag THEN 1 ELSE 2 END > 0"
        );
        assert_eq!(
            dd(r#"pl.when(pl.col("flag")).then(1) > 0"#),
            "CASE WHEN flag THEN 1 END > 0"
        );
        assert_eq!(
            dd(r#"pl.when(pl.col("flag")).then(1).when(pl.col("q3")).then(2).otherwise(3) > 0"#),
            "CASE WHEN flag THEN 1 WHEN q3 THEN 2 ELSE 3 END > 0"
        );
    }

    #[test]
    fn a_selection_reads_back_as_a_selection() {
        assert_eq!(
            dd(r#"pl.all_horizontal(pl.col("^.*(?:q[34]).*$").is_not_null())"#),
            "COLUMNS('q[34]') IS NOT NULL"
        );
        assert_eq!(
            dd(r#"pl.all_horizontal(pl.all().is_not_null())"#),
            "COLUMNS(*) IS NOT NULL"
        );
        assert_eq!(
            dd(r#"pl.all_horizontal(pl.col("q3", "q4").is_not_null())"#),
            "COLUMNS([q3, q4]) IS NOT NULL"
        );
    }

    #[test]
    fn an_unanchored_pattern_grows_the_wildcards_that_say_so() {
        assert_eq!(
            dd(r#"pl.col("s").str.contains("^(?:[a-z]+)$")"#),
            "s SIMILAR TO '[a-z]+'"
        );
        assert_eq!(
            dd(r#"pl.col("s").str.contains("abc")"#),
            "s SIMILAR TO '.*(?:abc).*'"
        );
    }

    #[test]
    fn aggregates_and_the_row_count() {
        assert_eq!(dd(r#"pl.col("qty").sum() > 0"#), "SUM(qty) > 0");
        assert_eq!(dd(r#"pl.col("qty").mean() > 0"#), "AVG(qty) > 0");
        assert_eq!(dd(r#"pl.col("s").count() > 0"#), "COUNT(s) > 0");
        assert_eq!(dd(r#"pl.len() > 0"#), "ROW_COUNT() > 0");
        assert_eq!(dd(r#"pl.col("flag").any()"#), "ANY(flag)");
        assert_eq!(
            dd(r#"pl.col("qty").round(2) == pl.col("qty")"#),
            "ROUND(qty, 2) = qty"
        );
        // `round(0)` is the language's one-argument form.
        assert_eq!(dd(r#"pl.col("qty").round(0) > 0"#), "ROUND(qty) > 0");
    }

    #[test]
    fn a_temporal_literal_reads_as_the_string_the_language_writes() {
        assert_eq!(
            dd(r#"pl.col("d") >= pl.lit(datetime.date(2000, 1, 1))"#),
            "d >= '2000-01-01'"
        );
        assert_eq!(
            dd(r#"pl.col("ts") >= pl.lit(datetime.datetime(2024, 1, 31, 9, 30, 0))"#),
            "ts >= '2024-01-31T09:30:00'"
        );
        assert_eq!(
            dd(r#"pl.col("ts") >= pl.lit(datetime.datetime.now()) - pl.duration(weeks=2)"#),
            "ts >= NOW() - interval(2, weeks)"
        );
    }

    #[test]
    fn a_construct_with_no_equivalent_names_itself() {
        assert!(refused(r#"pl.col("qty").cum_sum()"#).contains("`.cum_sum()`"));
        assert!(refused(r#"pl.sql_expr("x")"#).contains("`pl.sql_expr()`"));
        assert!(refused(r#"pl.col("flag") and pl.col("q3")"#).contains("`and`"));
        assert!(refused(r#"pl.col("qty") // pl.lit(2)"#).contains("`//`"));
        assert!(refused(r#"pl.col("qty") ** pl.lit(2)"#).contains("`**`"));
        assert!(refused(r#"qty > 0"#).contains("pl.col"));
        assert!(refused(r#"pl.col("qty").is_in(pl.col("n"))"#).contains("written-out list"));
    }
}
