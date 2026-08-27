//! `data-dict` — the language writing itself back out.
//!
//! The one target that is not a foreign language, and so the one that never
//! diverges: every construct has itself as its spelling. It exists because an
//! expression [read from another language](crate::parse) has to be shown in the
//! language it was read into, and because printing an expression the tool has
//! parsed is the clearest way to say what it understood.
//!
//! Being the language's own printer makes it the only target that should be its
//! own inverse: parsing what it emits gives back the tree it was given. Two
//! things stop that from being free, and both are handled here rather than left
//! to the reader. A `COLUMNS(...)` selection is kept symbolic instead of being
//! expanded, since expanding it would print a conjunction the author didn't
//! write. And a `LIKE` pattern is put back together from the pieces
//! [`lower`](crate::assert_expr::lower) took it apart into.

use chrono::SecondsFormat;

use super::{Ctx, Side, Target, Unsupported, prec};
use crate::assert_expr::{
    ArithOp, CmpOp, DatetimeConst, LikePattern, NodeKind, Selection, SelectorForm, TypedExpr,
    is_reserved, un_like_regex,
};

pub struct Canonical;

impl Target for Canonical {
    fn name(&self) -> &'static str {
        "data-dict"
    }

    fn prec(&self, e: &TypedExpr) -> u8 {
        match &e.kind {
            NodeKind::Or(..) => prec::OR,
            NodeKind::And(..) => prec::AND,
            NodeKind::Not(_) => prec::NOT,
            // Unlike DuckDB, which spells this as a function call, the language
            // has an infix `SIMILAR TO`, so it binds where a comparison does.
            NodeKind::Compare { .. }
            | NodeKind::IsNull { .. }
            | NodeKind::Between { .. }
            | NodeKind::In { .. }
            | NodeKind::Like { .. }
            | NodeKind::SimilarTo { .. } => prec::CMP,
            NodeKind::Arith { op, .. } => match op {
                ArithOp::Add | ArithOp::Sub => prec::ADD,
                ArithOp::Mul | ArithOp::Div => prec::MUL,
            },
            NodeKind::Neg(_) => prec::NEG,
            _ => prec::ATOM,
        }
    }

    fn column(&self, path: &[String]) -> String {
        path.iter()
            .map(|segment| name(segment))
            .collect::<Vec<_>>()
            .join(".")
    }

    fn conjunction(&self) -> (&'static str, u8) {
        ("AND", prec::AND)
    }

    /// Keep the selection symbolic. Every other target expands, because it has
    /// no `COLUMNS(...)` of its own; this one wrote the syntax, so expanding
    /// would print a rule the author didn't write.
    fn write_selection(
        &self,
        cx: &mut Ctx,
        selection: &Selection,
        root: &TypedExpr,
    ) -> Result<bool, Unsupported> {
        let selector = match &selection.form {
            SelectorForm::All => "COLUMNS(*)".to_string(),
            SelectorForm::Regex(pattern) => format!("COLUMNS({})", quote(pattern)),
            // The resolved columns are what the list named, in order.
            SelectorForm::List => format!(
                "COLUMNS([{}])",
                selection
                    .columns
                    .iter()
                    .map(|c| self.column(&c.path))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        cx.with_selected(selector, |cx| cx.free(root))?;
        Ok(true)
    }

    fn write(&self, cx: &mut Ctx, e: &TypedExpr) -> Result<(), Unsupported> {
        match &e.kind {
            NodeKind::Int(n) => cx.push(&n.to_string()),
            NodeKind::Float(x) => cx.push(&render_float(*x)),
            NodeKind::Str(s) => cx.push(&quote(s)),
            NodeKind::Bool(b) => cx.push(if *b { "TRUE" } else { "FALSE" }),
            NodeKind::Null => cx.push("NULL"),
            // The language has no temporal literal: a date is written as the
            // string it was read from, and becomes one again beside a `date`
            // column.
            NodeKind::Date(d) => cx.push(&quote(&d.to_string())),
            NodeKind::Datetime(t) => cx.push(&quote(&datetime(t))),
            NodeKind::Now => cx.push("NOW()"),
            NodeKind::Column(c) => cx.push(&self.column(&c.path)),
            NodeKind::Selected => {
                let reference = cx.selected().expect("a selection is in scope").to_string();
                cx.push(&reference);
            }
            NodeKind::Neg(x) => {
                cx.push("-");
                cx.child(prec::NEG, Side::Right, x)?;
            }
            NodeKind::Not(x) => {
                cx.push("NOT ");
                cx.child(prec::NOT, Side::Right, x)?;
            }
            NodeKind::And(l, r) => cx.infix(prec::AND, "AND", l, r)?,
            NodeKind::Or(l, r) => cx.infix(prec::OR, "OR", l, r)?,
            NodeKind::Arith { op, lhs, rhs } => {
                let (symbol, level) = match op {
                    ArithOp::Add => ("+", prec::ADD),
                    ArithOp::Sub => ("-", prec::ADD),
                    ArithOp::Mul => ("*", prec::MUL),
                    ArithOp::Div => ("/", prec::MUL),
                };
                cx.infix(level, symbol, lhs, rhs)?;
            }
            NodeKind::Compare { op, lhs, rhs } => {
                let symbol = match op {
                    CmpOp::Eq => "=",
                    CmpOp::Ne => "!=",
                    CmpOp::Lt => "<",
                    CmpOp::Le => "<=",
                    CmpOp::Gt => ">",
                    CmpOp::Ge => ">=",
                };
                cx.infix(prec::CMP, symbol, lhs, rhs)?;
            }
            NodeKind::IsNull { operand, negated } => {
                cx.child(prec::CMP, Side::Left, operand)?;
                cx.push(if *negated { " IS NOT NULL" } else { " IS NULL" });
            }
            NodeKind::Between {
                operand,
                lo,
                hi,
                negated,
            } => {
                cx.child(prec::CMP, Side::Left, operand)?;
                cx.push(if *negated {
                    " NOT BETWEEN "
                } else {
                    " BETWEEN "
                });
                cx.child(prec::CMP, Side::Right, lo)?;
                cx.push(" AND ");
                cx.child(prec::CMP, Side::Right, hi)?;
            }
            NodeKind::In {
                needle,
                haystack,
                negated,
            } => {
                cx.child(prec::CMP, Side::Left, needle)?;
                cx.push(if *negated { " NOT IN (" } else { " IN (" });
                cx.comma_separated(haystack, |cx, item| cx.free(item))?;
                cx.push(")");
            }
            NodeKind::Like {
                operand,
                pattern,
                negated,
            } => write_like(cx, operand, pattern, *negated)?,
            NodeKind::SimilarTo {
                operand,
                pattern,
                negated,
            } => {
                cx.child(prec::CMP, Side::Left, operand)?;
                cx.push(if *negated {
                    " NOT SIMILAR TO "
                } else {
                    " SIMILAR TO "
                });
                cx.child(prec::CMP, Side::Right, pattern)?;
            }
            NodeKind::Interval { n, unit } => {
                cx.push("interval(");
                cx.free(n)?;
                cx.push(&format!(", {})", unit.name()));
            }
            NodeKind::Case { whens, els } => {
                cx.push("CASE");
                for (cond, result) in whens {
                    cx.push(" WHEN ");
                    cx.free(cond)?;
                    cx.push(" THEN ");
                    cx.free(result)?;
                }
                if let Some(els) = els {
                    cx.push(" ELSE ");
                    cx.free(els)?;
                }
                cx.push(" END");
            }
            NodeKind::Func { op, args } => {
                let refs: Vec<&TypedExpr> = args.iter().collect();
                cx.call(op.name(), &refs)?;
            }
        }
        Ok(())
    }
}

/// `lower` takes a literal `LIKE` pattern apart; this puts it back. The three
/// special shapes rebuild directly, and the general one is un-escaped back into
/// its wildcards. A regex that didn't come from a `LIKE` pattern can only be
/// spelled `SIMILAR TO`, which is what it means anyway.
fn write_like(
    cx: &mut Ctx,
    operand: &TypedExpr,
    pattern: &LikePattern,
    negated: bool,
) -> Result<(), Unsupported> {
    let literal = match pattern {
        LikePattern::Exact(text) => Some(text.clone()),
        LikePattern::Prefix(text) => Some(format!("{text}%")),
        LikePattern::Suffix(text) => Some(format!("%{text}")),
        LikePattern::Regex(re) => un_like_regex(re),
        LikePattern::Dynamic(_) => None,
    };
    cx.child(prec::CMP, Side::Left, operand)?;
    match (literal, pattern) {
        (Some(text), _) => {
            cx.push(if negated { " NOT LIKE " } else { " LIKE " });
            cx.push(&quote(&text));
        }
        (None, LikePattern::Dynamic(expr)) => {
            cx.push(if negated { " NOT LIKE " } else { " LIKE " });
            cx.child(prec::CMP, Side::Right, expr)?;
        }
        (None, LikePattern::Regex(re)) => {
            cx.push(if negated {
                " NOT SIMILAR TO "
            } else {
                " SIMILAR TO "
            });
            cx.push(&quote(re));
        }
        (None, _) => unreachable!("every other pattern rebuilds a literal"),
    }
    Ok(())
}

/// A column name, backtick-quoted when a bare one wouldn't read back as itself:
/// a name that isn't an `IDENT`, or one that is but is [reserved](is_reserved).
fn name(text: &str) -> String {
    let ident = !text.is_empty()
        && text
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ident && !is_reserved(text) {
        text.to_string()
    } else {
        format!("`{}`", text.replace('`', "``"))
    }
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// A float that always reads back as one, so `2.0` doesn't become an integer.
/// The non-finite values are literals in this language, unlike in SQL.
fn render_float(x: f64) -> String {
    if x.is_nan() {
        return "NAN".to_string();
    }
    if x.is_infinite() {
        return if x.is_sign_negative() { "-INF" } else { "INF" }.to_string();
    }
    let text = x.to_string();
    if text.contains(['.', 'e', 'E']) {
        text
    } else {
        format!("{text}.0")
    }
}

/// ISO 8601 with the `T` separator, so the string reads back as a datetime —
/// and, for an offset-bearing one, *with* its offset. Dropping the offset (as
/// the R target does) would make the two constants print identically, so the
/// zoneless spelling would come back for both. `Z` is used for a zero offset,
/// which is how the spec writes it.
fn datetime(t: &DatetimeConst) -> String {
    match t {
        DatetimeConst::Offset(t) => t.to_rfc3339_opts(SecondsFormat::Secs, true),
        DatetimeConst::Naive(t) => t.format("%Y-%m-%dT%H:%M:%S").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::Canonical;
    use crate::assert_expr::{AssertExpr, Root, check_root, lower, tests::TestEnv};
    use crate::emit::emit;

    /// Print `source` in the language it is already written in.
    fn dd(source: &str) -> String {
        let expr = AssertExpr::parse(source).expect("parses");
        let findings = check_root(&expr, &TestEnv, Root::Any);
        assert!(findings.is_empty(), "{source:?}: {findings:?}");
        let ir = lower(&expr, &TestEnv).expect("lowers");
        emit(&Canonical, &ir).expect("emits").code
    }

    /// The printer is the language's own, so for a canonically-spelled
    /// expression it is the identity. That is a stronger claim than any
    /// individual mapping, and it is what makes the printer trustworthy as the
    /// way to show what a reader understood.
    #[track_caller]
    fn identity(source: &str) {
        assert_eq!(dd(source), source);
    }

    #[test]
    fn operators_and_literals_print_themselves() {
        identity("qty > 0");
        identity("qty = 42");
        identity("qty = 42.0");
        identity("qty = 0.5");
        identity("s = 'it''s'");
        identity("flag = TRUE");
        identity("qty IS NULL");
        identity("qty IS NOT NULL");
        identity("qty = INF");
        identity("qty = -INF");
        identity("qty = NAN");
        identity("n + 1 > 0");
        identity("n - 1 - 2 > 0");
        identity("n / qty > 1");
        identity("NOT flag");
        identity("q3 AND q4");
        identity("q3 OR q4");
    }

    #[test]
    fn predicates_print_themselves() {
        identity("qty IN (1, 2, 3)");
        identity("qty NOT IN (1, 2, 3)");
        identity("qty BETWEEN 0 AND 100");
        identity("qty NOT BETWEEN 0 AND 100");
        identity("s LIKE 'NZ-%'");
        identity("s LIKE '%.nz'");
        identity("s LIKE 'exact'");
        identity("s NOT LIKE 'NZ-%'");
        identity("s SIMILAR TO '[a-z]+'");
        identity("s NOT SIMILAR TO '[a-z]+'");
        identity("CASE WHEN flag THEN qty > 1 ELSE qty > 10 END");
        identity("CASE WHEN flag THEN qty > 1 END");
    }

    #[test]
    fn a_like_pattern_is_rebuilt_from_its_pieces() {
        // `lower` turns a wildcard in the middle into an anchored regex;
        // `un_like_regex` puts the wildcards back, so the round trip holds.
        identity("s LIKE 'a%b'");
        identity("s LIKE 'a_b'");
        identity("s LIKE 'a%b_c'");
        // A regex metacharacter in the pattern is escaped on the way in and
        // unescaped on the way out.
        identity("s LIKE 'a.b%c'");
        // A computed pattern was never taken apart.
        identity("s LIKE LOWER(postcode)");
    }

    #[test]
    fn functions_print_their_own_names() {
        identity("LENGTH(postcode) <= 10");
        identity("LOWER(s) = 'a'");
        identity("UPPER(s) = 'A'");
        identity("TRIM(s) = 'a'");
        identity("STARTS_WITH(s, 'NZ-')");
        identity("ENDS_WITH(s, '.nz')");
        identity("ABS(n) > 0");
        identity("FLOOR(qty) > 0");
        identity("CEIL(qty) > 0");
        identity("ROUND(qty) > 0");
        identity("ROUND(qty, 2) > 0");
        identity("MOD(n, 3) = 0");
        identity("IS_FINITE(qty)");
        identity("IS_INFINITE(qty)");
        identity("IS_NAN(qty)");
    }

    #[test]
    fn aggregates_print_their_own_names() {
        identity("SUM(qty) > 0");
        identity("AVG(qty) > 0");
        identity("MIN(qty) > 0");
        identity("MAX(qty) > 0");
        identity("COUNT(s) > 0");
        identity("ROW_COUNT() > 0");
        identity("COUNT_DISTINCT(s) <= 16");
        identity("ANY(flag)");
        identity("ALL(flag)");
        identity("qty <= 2 * MIN(qty)");
    }

    #[test]
    fn time_prints_itself() {
        identity("ts >= NOW() - interval(2, weeks)");
        identity("ts >= NOW() - interval(n, days)");
        identity("d >= '2000-01-01'");
        identity("ts >= '2024-01-31T09:30:00Z'");
        identity("d + interval(12, hours) < NOW()");
    }

    #[test]
    fn a_selection_stays_a_selection() {
        // Every other target expands this to a conjunction; the language wrote
        // the syntax, so it keeps it.
        identity("COLUMNS('q[34]') IS NOT NULL");
        identity("COLUMNS(*) IS NOT NULL");
        identity("COLUMNS([q3, q4]) IS NOT NULL");
        identity("COLUMNS('q[34]') IS NULL OR flag");
    }

    #[test]
    fn struct_fields_print_as_paths() {
        identity("LENGTH(addr.zip) > 0");
        identity("addr IS NOT NULL");
    }

    /// The test table has no awkward column names to reach through the whole
    /// pipeline, so the quoting rule is checked where it lives.
    #[test]
    fn a_name_is_quoted_when_a_bare_one_would_not_read_back() {
        assert_eq!(super::name("qty"), "qty");
        assert_eq!(super::name("_leading"), "_leading");
        assert_eq!(super::name("q3"), "q3");
        // Not an IDENT.
        assert_eq!(super::name("odd name"), "`odd name`");
        assert_eq!(super::name("3rd"), "`3rd`");
        assert_eq!(super::name(""), "``");
        assert_eq!(super::name("a-b"), "`a-b`");
        // An IDENT, but reserved, so bare it would read as a keyword or a
        // literal. Case-insensitively, as the keywords are.
        assert_eq!(super::name("inf"), "`inf`");
        assert_eq!(super::name("NaN"), "`NaN`");
        assert_eq!(super::name("case"), "`case`");
        // A backtick inside a name is doubled, as the grammar escapes it.
        assert_eq!(super::name("a`b"), "`a``b`");
    }

    #[test]
    fn parentheses_come_back_only_where_they_are_needed() {
        identity("NOT (q3 OR q4)");
        identity("(n + 1) * 2 > 0");
        identity("n - (1 - 2) > 0");
        // A unary operator nested in itself gains parentheses the source
        // didn't have, since it is a right operand of its own precedence.
        assert_eq!(dd("NOT NOT flag"), "NOT (NOT flag)");
        assert_eq!(dd("- -n > 0"), "-(-n) > 0");
        // Redundant parentheses are dropped: the tree doesn't record them.
        assert_eq!(dd("(qty) > (0)"), "qty > 0");
        assert_eq!(dd("(qty > 0) AND flag"), "qty > 0 AND flag");
    }

    #[test]
    fn the_printer_never_diverges() {
        // It is the language writing itself, so there is no edge to note.
        for source in ["qty > 0", "MOD(n, qty) = 0", "SUM(qty) > 0", "ANY(flag)"] {
            let expr = AssertExpr::parse(source).expect("parses");
            let ir = lower(&expr, &TestEnv).expect("lowers");
            assert!(emit(&Canonical, &ir).expect("emits").notes.is_empty());
        }
    }
}
