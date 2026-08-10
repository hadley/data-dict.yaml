//! Translating a checked assertion into another language.
//!
//! Every target reads the same [typed IR](crate::assert_expr::TypedAssertion)
//! and writes a **bare predicate** — the expression, spelled for that target,
//! for a caller to embed. What to do with it (filtering, reporting) is the
//! caller's business; see `site/expression-execution.md`.
//!
//! What is shared here is the part that is genuinely target-independent:
//! deciding where parentheses go. That is not cosmetic. Python's `&` and `|`
//! bind *tighter* than comparison where the language's `AND`/`OR` bind looser,
//! so a printer that reproduced the language's own precedence would emit
//! `a == 1 & b == 2` for Python and quietly mean `a == (1 & b) == 2`. Each
//! target therefore declares its own precedence, and [`Ctx::child`] parenthesises
//! against that rather than against the language's.
//!
//! # How the parenthesising works
//!
//! The IR is a tree, so it carries grouping in its shape and needs no
//! parentheses of its own; the printer's job is to put back exactly the
//! parentheses that make the flat text re-parse to that same shape, in the
//! target's grammar.
//!
//! Three pieces do it:
//!
//! 1. [`Target::prec`] maps a node to the precedence of the operator it will be
//!    spelled as, on the target's own scale ([`prec`] holds a default scale that
//!    suits the SQL family). It answers "how tightly does the text this node
//!    produces hold together?", which is a property of the spelling, not of the
//!    IR: a node emitted as a function call binds as [`prec::ATOM`] however
//!    loose the operator it came from — see DuckDB's `SIMILAR TO`.
//! 2. A `write` implementation never recurses directly. It descends through
//!    [`Ctx::child`], passing the precedence of the operator it is emitting and
//!    which [`Side`] the operand sits on. `child` asks the target for the
//!    child's own precedence and wraps when the surrounding operator binds more
//!    tightly than the child, or binds equally tightly and the child is on the
//!    right — the latter because these operators are left-associative, so
//!    `a - (b - c)` is not `a - b - c`.
//! 3. Positions that are already delimited — a function argument, a list item, a
//!    `CASE` branch — pass [`prec::DELIMITED`], which is below every operator
//!    and so never wraps. [`Ctx::free`] is that case.
//!
//! Because the decision is local (one parent precedence, one child precedence, a
//! side) a target only has to be honest in `prec` and consistent about routing
//! through `child`; it never reasons about parentheses itself.

mod duckdb;

use std::collections::BTreeSet;

use crate::assert_expr::{ColumnRef, TypedAssertion, TypedExpr};

pub use duckdb::DuckDb;

/// How faithfully a construct translates. Every (construct, target) pair has
/// one, and the differential tests hold the first two to their word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fidelity {
    /// The same result on every input.
    Exact,
    /// Exact, but only because the translation adds code the expression didn't
    /// ask for. The guard is part of the mapping.
    Guarded,
    /// Agrees except on a documented edge. Using it attaches the note.
    Divergent(&'static str),
}

/// A construct this target cannot express. Refusal is per (expression, target):
/// every other target still translates.
#[derive(Debug, Clone)]
pub struct Unsupported {
    pub what: &'static str,
    pub why: &'static str,
}

/// A translated predicate, with any notes the constructs it used attached.
#[derive(Debug, Clone)]
pub struct Emitted {
    pub code: String,
    pub notes: Vec<&'static str>,
}

/// Precedence levels, loosest to tightest. A target that orders its operators
/// differently says so in [`Target::prec`]; these are only the defaults that
/// suit the SQL family.
pub mod prec {
    pub const OR: u8 = 1;
    pub const AND: u8 = 2;
    pub const NOT: u8 = 3;
    pub const CMP: u8 = 4;
    pub const ADD: u8 = 5;
    pub const MUL: u8 = 6;
    pub const NEG: u8 = 7;
    /// A literal, a column, a call, or anything else that never needs wrapping.
    pub const ATOM: u8 = 8;
    /// A position that delimits its own contents — a function argument, a list
    /// item, a `CASE` branch — where no operand can be regrouped and so no
    /// parentheses are ever needed.
    pub const DELIMITED: u8 = 0;
}

/// Which operand of a binary operator a child is, so associativity can decide
/// whether an equal-precedence child needs parentheses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    /// Not an operand of a binary operator — a function argument, say, which is
    /// already delimited.
    Free,
}

pub trait Target {
    /// The target's name as `family(dialect)`.
    fn name(&self) -> &'static str;

    /// The precedence of the operator `e` emits as, in this target's own order.
    fn prec(&self, e: &TypedExpr) -> u8;

    /// A column reference, in this family's convention.
    fn column(&self, path: &[String]) -> String;

    /// How the columns of a `COLUMNS(...)` are combined when the target has no
    /// idiom of its own and the selection is expanded. Returns the operator and
    /// its precedence.
    fn conjunction(&self) -> (&'static str, u8);

    /// Write `e`. Recurse through [`Ctx::child`] so parentheses are handled.
    fn write(&self, cx: &mut Ctx, e: &TypedExpr) -> Result<(), Unsupported>;
}

/// The output being built, and the target building it.
pub struct Ctx<'t> {
    target: &'t dyn Target,
    out: String,
    notes: BTreeSet<&'static str>,
    /// The column `Selected` currently stands for, when a selection is being
    /// expanded one column at a time.
    selected: Option<&'t ColumnRef>,
}

impl<'t> Ctx<'t> {
    pub fn push(&mut self, text: &str) {
        self.out.push_str(text);
    }

    /// Record a construct's fidelity, so a [`Fidelity::Divergent`] mapping
    /// attaches its note by being used rather than by being remembered.
    pub fn fidelity(&mut self, fidelity: Fidelity) {
        if let Fidelity::Divergent(note) = fidelity {
            self.notes.insert(note);
        }
    }

    /// The column `Selected` stands for right now.
    pub fn selected(&self) -> Option<&ColumnRef> {
        self.selected
    }

    /// Emit `e` as an operand of an operator with precedence `parent`,
    /// parenthesising when the target's own precedence would otherwise regroup
    /// it. An equal-precedence operand needs parentheses on the side
    /// associativity doesn't favour — `a - (b - c)` is not `a - b - c`.
    pub fn child(&mut self, parent: u8, side: Side, e: &TypedExpr) -> Result<(), Unsupported> {
        let own = self.target.prec(e);
        let wrap = own < parent || (own == parent && side == Side::Right);
        if wrap {
            self.out.push('(');
        }
        self.target.write(self, e)?;
        if wrap {
            self.out.push(')');
        }
        Ok(())
    }

    /// Emit `e` in a position that delimits it — a function argument, a list
    /// item, a `CASE` branch — so it never needs parentheses.
    pub fn free(&mut self, e: &TypedExpr) -> Result<(), Unsupported> {
        self.child(prec::DELIMITED, Side::Free, e)
    }

    /// `name(arg, arg, …)`.
    pub fn call(&mut self, name: &str, args: &[&TypedExpr]) -> Result<(), Unsupported> {
        self.push(name);
        self.push("(");
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            self.free(arg)?;
        }
        self.push(")");
        Ok(())
    }

    /// `lhs <op> rhs`, at `parent` precedence.
    pub fn infix(
        &mut self,
        parent: u8,
        op: &str,
        lhs: &TypedExpr,
        rhs: &TypedExpr,
    ) -> Result<(), Unsupported> {
        self.child(parent, Side::Left, lhs)?;
        self.push(" ");
        self.push(op);
        self.push(" ");
        self.child(parent, Side::Right, rhs)
    }
}

/// Translate a whole assertion, expanding a `COLUMNS(...)` selection into a
/// conjunction over the columns it resolved to.
///
/// Expansion is always available and always correct; a target with a
/// self-contained multi-column idiom overrides this. See the spec's
/// [note on why DuckDB expands](https://data-dict.tidyverse.org/expression-execution.html#selecting-multiple-columns).
pub fn emit(target: &dyn Target, assertion: &TypedAssertion) -> Result<Emitted, Unsupported> {
    let mut cx = Ctx {
        target,
        out: String::new(),
        notes: BTreeSet::new(),
        selected: None,
    };
    match &assertion.selection {
        None => target.write(&mut cx, &assertion.root)?,
        Some(selection) => {
            let (op, prec) = target.conjunction();
            for (i, column) in selection.columns.iter().enumerate() {
                if i > 0 {
                    cx.push(" ");
                    cx.push(op);
                    cx.push(" ");
                }
                cx.selected = Some(column);
                cx.child(prec, Side::Free, &assertion.root)?;
            }
            cx.selected = None;
        }
    }
    Ok(Emitted {
        code: cx.out,
        notes: cx.notes.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_expr::{AssertExpr, lower, tests::TestEnv};

    /// Translate an expression to DuckDB, panicking if it doesn't check.
    fn sql(source: &str) -> String {
        let expr = AssertExpr::parse(source).expect("parses");
        let findings = crate::assert_expr::check(&expr, &TestEnv);
        assert!(findings.is_empty(), "{source:?}: {findings:?}");
        let ir = lower(&expr, &TestEnv).expect("lowers");
        emit(&DuckDb, &ir).expect("emits").code
    }

    fn notes(source: &str) -> Vec<&'static str> {
        let expr = AssertExpr::parse(source).expect("parses");
        let ir = lower(&expr, &TestEnv).expect("lowers");
        emit(&DuckDb, &ir).expect("emits").notes
    }

    #[test]
    fn columns_are_quoted() {
        assert_eq!(sql("qty > 0"), r#""qty" > 0"#);
        // A struct field is a path, each segment quoted.
        assert_eq!(sql("LENGTH(addr.zip) > 0"), r#"length("addr"."zip") > 0"#);
    }

    #[test]
    fn parentheses_appear_only_where_they_change_the_reading() {
        // Tighter-binding children need none.
        assert_eq!(sql("qty > 0 AND flag"), r#""qty" > 0 AND "flag""#);
        // A looser-binding child does.
        assert_eq!(sql("NOT (q3 OR q4)"), r#"NOT ("q3" OR "q4")"#);
        assert_eq!(sql("(n + 1) * 2 > 0"), r#"("n" + 1) * 2 > 0"#);
        // Left-associative operators need none on the left, and do on the right.
        assert_eq!(sql("n - 1 - 2 > 0"), r#""n" - 1 - 2 > 0"#);
        assert_eq!(sql("n - (1 - 2) > 0"), r#""n" - (1 - 2) > 0"#);
        // A unary operator nested in itself is a right-hand operand of its own
        // precedence, so the output gains parentheses the source didn't have.
        assert_eq!(sql("NOT NOT flag"), r#"NOT (NOT "flag")"#);
        assert_eq!(sql("- -n > 0"), r#"-(-"n") > 0"#);
    }

    #[test]
    fn literals_keep_their_representation() {
        assert_eq!(sql("qty = 42"), r#""qty" = 42"#);
        // A float stays a float, so `2.0` isn't read as an integer.
        assert_eq!(sql("qty = 42.0"), r#""qty" = 42.0"#);
        assert_eq!(sql("qty = 0.5"), r#""qty" = 0.5"#);
        assert_eq!(sql("s = 'it''s'"), r#""s" = 'it''s'"#);
        assert_eq!(sql("flag = TRUE"), r#""flag" = TRUE"#);
        assert_eq!(sql("qty IS NULL"), r#""qty" IS NULL"#);
    }

    #[test]
    fn a_temporal_literal_is_constructed_not_coerced() {
        assert_eq!(sql("d >= '2000-01-01'"), r#""d" >= DATE '2000-01-01'"#);
        assert_eq!(
            sql("ts >= '2024-01-31T09:30:00Z'"),
            r#""ts" >= TIMESTAMP '2024-01-31 09:30:00'"#
        );
    }

    #[test]
    fn like_uses_the_clearest_native_form() {
        assert_eq!(sql("s LIKE 'NZ-%'"), r#"starts_with("s", 'NZ-')"#);
        assert_eq!(sql("s LIKE '%.nz'"), r#"ends_with("s", '.nz')"#);
        assert_eq!(sql("s LIKE 'exact'"), r#""s" = 'exact'"#);
        assert_eq!(sql("s LIKE 'a%b'"), r#"regexp_full_match("s", '^a.*b$')"#);
        // DuckDB's own LIKE takes a computed pattern, so nothing is refused.
        assert_eq!(
            sql("s LIKE LOWER(postcode)"),
            r#""s" LIKE lower("postcode")"#
        );
        assert_eq!(sql("s NOT LIKE 'NZ-%'"), r#"NOT starts_with("s", 'NZ-')"#);
    }

    #[test]
    fn similar_to_is_an_anchored_regex_match() {
        assert_eq!(
            sql("s SIMILAR TO '[a-z]+'"),
            r#"regexp_full_match("s", '[a-z]+')"#
        );
    }

    #[test]
    fn aggregates_use_the_native_spellings() {
        assert_eq!(sql("SUM(qty) > 0"), r#"sum("qty") > 0"#);
        assert_eq!(sql("ROW_COUNT() > 0"), "count(*) > 0");
        assert_eq!(
            sql("COUNT_DISTINCT(s) <= 16"),
            r#"count(DISTINCT "s") <= 16"#
        );
        // `bool_or`/`bool_and` return null on all-null input, as the language does.
        assert_eq!(sql("ANY(flag)"), r#"bool_or("flag")"#);
        assert_eq!(sql("ALL(flag)"), r#"bool_and("flag")"#);
    }

    #[test]
    fn an_interval_folds_into_a_literal_when_it_can() {
        assert_eq!(
            sql("ts >= NOW() - interval(2, weeks)"),
            r#""ts" >= current_timestamp - INTERVAL '2 weeks'"#
        );
        // A computed count has to multiply a unit interval instead.
        assert_eq!(
            sql("ts >= NOW() - interval(n, days)"),
            r#""ts" >= current_timestamp - ("n" * INTERVAL '1 days')"#
        );
    }

    #[test]
    fn case_and_membership() {
        assert_eq!(sql("qty IN (1, 2, 3)"), r#""qty" IN (1, 2, 3)"#);
        assert_eq!(sql("qty BETWEEN 0 AND 100"), r#""qty" BETWEEN 0 AND 100"#);
        assert_eq!(
            sql("CASE WHEN flag THEN qty > 1 ELSE qty > 10 END"),
            r#"CASE WHEN "flag" THEN "qty" > 1 ELSE "qty" > 10 END"#
        );
    }

    #[test]
    fn a_delimited_position_needs_no_parentheses() {
        // Commas and keywords already say where the operand ends.
        assert_eq!(sql("ABS(n + 1) > 0"), r#"abs("n" + 1) > 0"#);
        assert_eq!(sql("qty IN (n + 1, 2)"), r#""qty" IN ("n" + 1, 2)"#);
    }

    #[test]
    fn a_selection_expands_to_a_conjunction() {
        assert_eq!(
            sql("COLUMNS('q[34]') IS NOT NULL"),
            r#""q3" IS NOT NULL AND "q4" IS NOT NULL"#
        );
        // Each copy of a root looser than the conjunction is parenthesised, so
        // the `OR` isn't regrouped across the columns.
        assert_eq!(
            sql("COLUMNS('q[34]') IS NULL OR flag"),
            r#"("q3" IS NULL OR "flag") AND ("q4" IS NULL OR "flag")"#
        );
    }

    #[test]
    fn divergences_attach_a_note_by_being_used() {
        assert!(notes("qty > 0").is_empty());
        assert!(notes("n / qty > 1")[0].contains("infinity"));
        assert!(notes("MOD(n, qty) = 0")[0].contains("zero modulus"));
        assert!(notes("SUM(qty) > 0")[0].contains("128 bits"));
    }
}
