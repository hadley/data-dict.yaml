//! `Python(polars)`.
//!
//! The closest target to the language after DuckDB, and closer than R in the
//! places that matter most. polars keeps null and NaN apart the way the language
//! does — `is_null` is false for a NaN, and `is_nan`, `is_in` and every string
//! method propagate a null rather than answering `False` — so almost none of the
//! guards R needs are needed here. It matches RE2, it gives an infinity or a NaN
//! for a zero divisor, and its `%` takes the remainder's sign from the divisor,
//! all as the language specifies.
//!
//! Four things differ, and each is stated where it is used: a NaN compares as an
//! ordinary value (and equals itself), `round` goes to even on a half, `sum`,
//! `any` and `all` fold an empty column to their identity where the language
//! returns null, and integer arithmetic wraps at 64 bits where the language
//! [reports an overflow](https://data-dict.tidyverse.org/expression-execution.html#no-result).
//! Two more are guarded rather than noted: `n_unique` counts a null as a value,
//! and adding a duration to a date keeps it a date.
//!
//! # Precedence
//!
//! Python is the reason [`Target::prec`](super::Target::prec) exists. Its `&`
//! and `|` are the bitwise operators, which bind **tighter** than comparison —
//! the reverse of the language's `AND`/`OR`. A printer that used the language's
//! own precedence would emit `pl.col("a") == 1 & pl.col("b") == 2`, which Python
//! reads as `a == (1 & b) == 2`: a different expression, and one polars rejects
//! at run time rather than quietly answering wrongly. The table below puts
//! comparison at the bottom, so every comparison under a `&` or `|` is
//! parenthesised.

use super::{Ctx, Fidelity, Side, Target, Unsupported};
use crate::assert_expr::{
    ArithOp, CmpOp, DatetimeConst, IntervalUnit, LikePattern, NodeKind, Op, Selection,
    SelectorForm, Type, TypedExpr,
};

pub struct Polars;

/// Python's precedence, loosest first. Comparison is the loosest of the
/// operators used here, and `&`/`|` bind tighter than it — see the module note.
mod p {
    pub const CMP: u8 = 1;
    pub const OR: u8 = 2;
    pub const AND: u8 = 3;
    pub const ADD: u8 = 4;
    pub const MUL: u8 = 5;
    /// Unary `-` and `~`, which bind tighter than any binary operator here.
    pub const NEG: u8 = 6;
    pub const ATOM: u8 = 7;
}

impl Target for Polars {
    fn name(&self) -> &'static str {
        "Python(polars)"
    }

    fn prec(&self, e: &TypedExpr) -> u8 {
        match &e.kind {
            NodeKind::Compare { .. } => p::CMP,
            NodeKind::Or(..) => p::OR,
            NodeKind::And(..) => p::AND,
            NodeKind::Arith { op, .. } => match op {
                ArithOp::Add | ArithOp::Sub => p::ADD,
                ArithOp::Mul | ArithOp::Div => p::MUL,
            },
            NodeKind::Neg(_) | NodeKind::Not(_) => p::NEG,
            // Everything else is a method call or a function call, which holds
            // together however loose the operator it came from.
            _ => p::ATOM,
        }
    }

    fn column(&self, path: &[String]) -> String {
        // A struct field is reached inside the column that holds it.
        let mut out = format!("pl.col({})", string(&path[0]));
        for field in &path[1..] {
            out.push_str(&format!(".struct.field({})", string(field)));
        }
        out
    }

    fn conjunction(&self) -> (&'static str, u8) {
        ("&", p::AND)
    }

    /// polars has `all_horizontal`, which is an ordinary expression with the
    /// language's own combination and null semantics, so the selection stays a
    /// selection rather than being expanded.
    fn write_selection(
        &self,
        cx: &mut Ctx,
        selection: &Selection,
        root: &TypedExpr,
    ) -> Result<bool, Unsupported> {
        let selector = match &selection.form {
            SelectorForm::All => "pl.all()".to_string(),
            // polars anchors a column pattern; the language's is unanchored, so
            // it is wrapped to match anywhere in the name.
            SelectorForm::Regex(pattern) => {
                format!("pl.col({})", string(&format!("^.*(?:{pattern}).*$")))
            }
            SelectorForm::List => format!(
                "pl.col({})",
                selection
                    .columns
                    .iter()
                    .map(|c| string(&c.path[0]))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        cx.push("pl.all_horizontal(");
        cx.with_selected(selector, |cx| cx.free(root))?;
        cx.push(")");
        Ok(true)
    }

    fn write(&self, cx: &mut Ctx, e: &TypedExpr) -> Result<(), Unsupported> {
        match &e.kind {
            NodeKind::Int(n) => cx.push(&format!("pl.lit({n})")),
            NodeKind::Float(x) => cx.push(&render_float(*x)),
            NodeKind::Str(s) => cx.push(&format!("pl.lit({})", string(s))),
            NodeKind::Bool(b) => cx.push(if *b { "pl.lit(True)" } else { "pl.lit(False)" }),
            NodeKind::Null => cx.push("pl.lit(None)"),
            NodeKind::Date(d) => cx.push(&format!(
                "pl.lit(datetime.date({}, {}, {}))",
                d.format("%Y"),
                d.format("%-m"),
                d.format("%-d")
            )),
            NodeKind::Datetime(t) => cx.push(&datetime(t)),
            NodeKind::Now => cx.push("pl.lit(datetime.datetime.now())"),
            NodeKind::Column(c) => cx.push(&self.column(&c.path)),
            NodeKind::Selected => {
                let reference = cx.selected().expect("a selection is in scope").to_string();
                cx.push(&reference);
            }
            NodeKind::Neg(x) => {
                cx.push("-");
                cx.child(p::NEG, Side::Right, x)?;
            }
            NodeKind::Not(x) => {
                cx.push("~");
                cx.child(p::NEG, Side::Right, x)?;
            }
            NodeKind::And(l, r) => cx.infix(p::AND, "&", l, r)?,
            NodeKind::Or(l, r) => cx.infix(p::OR, "|", l, r)?,
            NodeKind::Arith { op, lhs, rhs } => {
                if is_interval_shift(lhs, rhs) {
                    return write_interval_shift(cx, *op, lhs, rhs);
                }
                if matches!(op, ArithOp::Add | ArithOp::Sub | ArithOp::Mul)
                    && (lhs.ty == Type::Number || rhs.ty == Type::Number)
                {
                    cx.fidelity(OVERFLOW);
                }
                let (symbol, level) = match op {
                    ArithOp::Add => ("+", p::ADD),
                    ArithOp::Sub => ("-", p::ADD),
                    ArithOp::Mul => ("*", p::MUL),
                    ArithOp::Div => ("/", p::MUL),
                };
                cx.infix(level, symbol, lhs, rhs)?;
            }
            NodeKind::Compare { op, lhs, rhs } => {
                if super::over_numbers(&[lhs, rhs]) {
                    cx.fidelity(NAN_COMPARISON);
                }
                let symbol = match op {
                    CmpOp::Eq => "==",
                    CmpOp::Ne => "!=",
                    CmpOp::Lt => "<",
                    CmpOp::Le => "<=",
                    CmpOp::Gt => ">",
                    CmpOp::Ge => ">=",
                };
                cx.infix(p::CMP, symbol, lhs, rhs)?;
            }
            // `is_null` is false for a NaN, exactly as the language says.
            NodeKind::IsNull { operand, negated } => {
                method(
                    cx,
                    operand,
                    if *negated { "is_not_null" } else { "is_null" },
                    &[],
                )?;
            }
            NodeKind::Between {
                operand,
                lo,
                hi,
                negated,
            } => {
                if super::over_numbers(&[operand, lo, hi]) {
                    cx.fidelity(NAN_COMPARISON);
                }
                if *negated {
                    cx.push("~");
                }
                method(cx, operand, "is_between", &[lo, hi])?;
            }
            NodeKind::In {
                needle,
                haystack,
                negated,
            } => {
                if super::over_numbers(&[needle]) {
                    cx.fidelity(NAN_COMPARISON);
                }
                if *negated {
                    cx.push("~");
                }
                // `is_in` propagates a null subject, as the language does, so
                // no guard is needed — unlike R's `%in%`.
                cx.child(p::ATOM, Side::Left, needle)?;
                cx.push(".is_in(");
                write_haystack(cx, haystack)?;
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
                // RE2 both sides; `contains` matches anywhere, so the pattern
                // is anchored to match the whole string.
                if *negated {
                    cx.push("~");
                }
                cx.child(p::ATOM, Side::Left, operand)?;
                cx.push(".str.contains(");
                anchored(cx, pattern)?;
                cx.push(")");
            }
            NodeKind::Interval { n, unit } => write_interval(cx, n, *unit)?,
            NodeKind::Case { whens, els } => {
                for (i, (cond, result)) in whens.iter().enumerate() {
                    cx.push(if i == 0 { "pl.when(" } else { ".when(" });
                    cx.free(cond)?;
                    cx.push(").then(");
                    cx.free(result)?;
                    cx.push(")");
                }
                // Without `otherwise` polars gives null, which is what an
                // `ELSE`-less `CASE` means.
                if let Some(els) = els {
                    cx.push(".otherwise(");
                    cx.free(els)?;
                    cx.push(")");
                }
            }
            NodeKind::Func { op, args } => write_func(cx, *op, args)?,
        }
        Ok(())
    }
}

/// polars makes a NaN equal to itself and orders it above every number, where
/// the language leaves it unordered and answers `false`. Nothing in an
/// expression can recover that.
/// Where polars and the language disagree, stated once.
///
/// Each is a fact about the two languages rather than a direction of travel, so
/// [reading polars](crate::parse::python) attaches the same words.
pub(crate) mod notes {
    pub const NAN_COMPARISON: &str = "polars compares a NaN as an ordinary value and makes it equal to itself, where data-dict answers false for every comparison a NaN reaches; a row holding one passes here and is reported there.";

    pub const ROUNDING: &str = "polars rounds halves to even, where data-dict rounds them away from zero, so results differ on an exact half.";

    /// `sum`, `any` and `all` return their fold's identity for an empty or
    /// all-null column; `mean`, `min` and `max` return null, as the language does.
    pub const EMPTY_FOLD: &str = "polars folds an empty or all-null column to the identity (0 for `sum`, False for `any`, True for `all`) where data-dict returns null, so such an assertion differs on an empty table.";

    pub const OVERFLOW: &str = "polars wraps integer arithmetic at 64 bits, where data-dict reports the overflow (D09) and withdraws the verdict rather than producing a wrong value.";

    /// Only a reader meets this one: the emitter always drops the nulls first.
    pub const COUNTS_NULL: &str = "`n_unique` counts a null as one of the distinct values, where data-dict's `COUNT_DISTINCT` skips them; write `.drop_nulls().n_unique()` to mean what data-dict means.";
}

const NAN_COMPARISON: Fidelity = Fidelity::Divergent(notes::NAN_COMPARISON);

const ROUNDING: Fidelity = Fidelity::Divergent(notes::ROUNDING);

const EMPTY_FOLD: Fidelity = Fidelity::Divergent(notes::EMPTY_FOLD);

const OVERFLOW: Fidelity = Fidelity::Divergent(notes::OVERFLOW);

/// Emit `receiver.name(args…)`, parenthesising the receiver when it isn't
/// already an atom — `(pl.col("a") + 1).is_null()`.
fn method(
    cx: &mut Ctx,
    receiver: &TypedExpr,
    name: &str,
    args: &[&TypedExpr],
) -> Result<(), Unsupported> {
    cx.child(p::ATOM, Side::Left, receiver)?;
    cx.push(".");
    cx.push(name);
    cx.push("(");
    cx.comma_separated(args, |cx, arg| cx.free(arg))?;
    cx.push(")");
    Ok(())
}

/// `x.str.starts_with(p)` and its `ends` sibling. A literal pattern is written
/// bare rather than wrapped in `pl.lit`, which reads better and — since the
/// `LIKE` decomposition reaches the same method — keeps one spelling for one
/// meaning.
fn affix(
    cx: &mut Ctx,
    subject: &TypedExpr,
    name: &str,
    pattern: &TypedExpr,
) -> Result<(), Unsupported> {
    cx.child(p::ATOM, Side::Left, subject)?;
    cx.push(".");
    cx.push(name);
    cx.push("(");
    match literal(pattern) {
        Some(value) => cx.push(&value),
        None => cx.free(pattern)?,
    }
    cx.push(")");
    Ok(())
}

/// The right-hand side of an `is_in`.
///
/// polars will not take a list of expressions, so a list of literals is written
/// as a plain Python list — much the clearer form, and the ordinary case — and
/// anything computed is gathered with `concat_list`, which does take them.
fn write_haystack(cx: &mut Ctx, haystack: &[TypedExpr]) -> Result<(), Unsupported> {
    if let Some(values) = haystack.iter().map(literal).collect::<Option<Vec<_>>>() {
        cx.push(&format!("[{}]", values.join(", ")));
        return Ok(());
    }
    cx.push("pl.concat_list([");
    cx.comma_separated(haystack, |cx, item| cx.free(item))?;
    cx.push("])");
    Ok(())
}

/// A node's value as a bare Python literal, for the positions polars wants one
/// in rather than an expression. `None` for anything computed.
fn literal(e: &TypedExpr) -> Option<String> {
    Some(match &e.kind {
        NodeKind::Int(n) => n.to_string(),
        NodeKind::Float(x) => bare_float(*x),
        NodeKind::Str(s) => string(s),
        NodeKind::Bool(b) => (if *b { "True" } else { "False" }).to_string(),
        NodeKind::Null => "None".to_string(),
        _ => return None,
    })
}

fn is_interval_shift(lhs: &TypedExpr, rhs: &TypedExpr) -> bool {
    lhs.ty == Type::Interval || rhs.ty == Type::Interval
}

/// A `Date` plus a duration stays a `Date` in polars, silently dropping
/// anything shorter than a day; the language makes it a datetime. Casting the
/// date first restores that, so the mapping is exact rather than approximate.
fn write_interval_shift(
    cx: &mut Ctx,
    op: ArithOp,
    lhs: &TypedExpr,
    rhs: &TypedExpr,
) -> Result<(), Unsupported> {
    let (base, duration) = if lhs.ty == Type::Interval {
        (rhs, lhs)
    } else {
        (lhs, rhs)
    };
    if base.ty == Type::Date {
        cx.child(p::ATOM, Side::Left, base)?;
        cx.push(".cast(pl.Datetime(\"us\"))");
    } else {
        cx.child(p::ADD, Side::Left, base)?;
    }
    cx.push(if op == ArithOp::Sub { " - " } else { " + " });
    cx.child(p::ADD, Side::Right, duration)?;
    Ok(())
}

/// `pl.duration(unit=n)`. Every one of the language's units is one polars takes.
fn write_interval(cx: &mut Ctx, n: &TypedExpr, unit: IntervalUnit) -> Result<(), Unsupported> {
    let keyword = match unit {
        IntervalUnit::Seconds => "seconds",
        IntervalUnit::Minutes => "minutes",
        IntervalUnit::Hours => "hours",
        IntervalUnit::Days => "days",
        IntervalUnit::Weeks => "weeks",
    };
    cx.push("pl.duration(");
    cx.push(keyword);
    cx.push("=");
    cx.free(n)?;
    cx.push(")");
    Ok(())
}

fn write_like(
    cx: &mut Ctx,
    operand: &TypedExpr,
    pattern: &LikePattern,
    negated: bool,
) -> Result<(), Unsupported> {
    if negated {
        cx.push("~");
    }
    match pattern {
        // Equality is clearer than a one-branch pattern, and propagates a null
        // the same way.
        LikePattern::Exact(text) => {
            cx.child(p::CMP, Side::Left, operand)?;
            cx.push(&format!(" == pl.lit({})", string(text)));
        }
        LikePattern::Prefix(text) => {
            cx.child(p::ATOM, Side::Left, operand)?;
            cx.push(&format!(".str.starts_with({})", string(text)));
        }
        LikePattern::Suffix(text) => {
            cx.child(p::ATOM, Side::Left, operand)?;
            cx.push(&format!(".str.ends_with({})", string(text)));
        }
        LikePattern::Regex(re) => {
            cx.child(p::ATOM, Side::Left, operand)?;
            cx.push(&format!(".str.contains({})", string(re)));
        }
        // polars takes a pattern column, so a computed one needs no run-time
        // translation — but the language's wildcards would have to become a
        // regex, and there is nothing that does it at run time.
        LikePattern::Dynamic(_) => {
            return Err(Unsupported {
                what: "`LIKE` with a computed pattern",
                why: "the pattern must be a literal so its wildcards can be translated; \
                      polars has no run-time equivalent",
            });
        }
    }
    Ok(())
}

/// `SIMILAR TO` matches the whole string; `str.contains` matches anywhere.
fn anchored(cx: &mut Ctx, pattern: &TypedExpr) -> Result<(), Unsupported> {
    if let NodeKind::Str(text) = &pattern.kind {
        cx.push(&string(&format!("^(?:{text})$")));
        return Ok(());
    }
    cx.push("pl.lit(\"^(?:\") + ");
    cx.child(p::ADD, Side::Right, pattern)?;
    cx.push(" + pl.lit(\")$\")");
    Ok(())
}

fn write_func(cx: &mut Ctx, op: Op, args: &[TypedExpr]) -> Result<(), Unsupported> {
    match op {
        Op::Length => method(cx, &args[0], "str.len_chars", &[])?,
        Op::Lower => method(cx, &args[0], "str.to_lowercase", &[])?,
        Op::Upper => method(cx, &args[0], "str.to_uppercase", &[])?,
        Op::Trim => method(cx, &args[0], "str.strip_chars", &[])?,
        Op::StartsWith => affix(cx, &args[0], "str.starts_with", &args[1])?,
        Op::EndsWith => affix(cx, &args[0], "str.ends_with", &args[1])?,
        Op::Abs => method(cx, &args[0], "abs", &[])?,
        Op::Floor => method(cx, &args[0], "floor", &[])?,
        Op::Ceil => method(cx, &args[0], "ceil", &[])?,
        Op::Round => {
            cx.fidelity(ROUNDING);
            // polars takes the digit count as a plain Python integer, not an
            // expression, so a computed one has nowhere to go. The language's
            // one-argument form means zero digits.
            let digits = match args.get(1) {
                None => 0,
                Some(TypedExpr {
                    kind: NodeKind::Int(n),
                    ..
                }) => *n,
                Some(_) => {
                    return Err(Unsupported {
                        what: "`ROUND` with a computed number of digits",
                        why: "polars takes the digit count as a plain integer, not an \
                              expression, so it must be a literal",
                    });
                }
            };
            cx.child(p::ATOM, Side::Left, &args[0])?;
            cx.push(&format!(".round({digits})"));
        }
        // polars' `%` takes the remainder's sign from the divisor, as the
        // language does.
        Op::Mod => cx.infix(p::MUL, "%", &args[0], &args[1])?,
        Op::IsFinite => method(cx, &args[0], "is_finite", &[])?,
        Op::IsInfinite => method(cx, &args[0], "is_infinite", &[])?,
        Op::IsNan => method(cx, &args[0], "is_nan", &[])?,
        // `mean`, `min` and `max` give null for an empty column, as the
        // language does, so only the three that fold to an identity diverge.
        Op::Min => method(cx, &args[0], "min", &[])?,
        Op::Max => method(cx, &args[0], "max", &[])?,
        Op::Avg => method(cx, &args[0], "mean", &[])?,
        Op::Sum => {
            cx.fidelity(EMPTY_FOLD);
            cx.fidelity(OVERFLOW);
            method(cx, &args[0], "sum", &[])?;
        }
        Op::Any => {
            cx.fidelity(EMPTY_FOLD);
            method(cx, &args[0], "any", &[])?;
        }
        Op::All => {
            cx.fidelity(EMPTY_FOLD);
            method(cx, &args[0], "all", &[])?;
        }
        // `count` counts the non-null values, as the language does.
        Op::Count => method(cx, &args[0], "count", &[])?,
        Op::RowCount => cx.push("pl.len()"),
        // `n_unique` counts a null as one of the distinct values, so the nulls
        // are dropped first. The guard is part of the mapping.
        Op::CountDistinct => {
            cx.child(p::ATOM, Side::Left, &args[0])?;
            cx.push(".drop_nulls().n_unique()");
        }
    }
    Ok(())
}

/// A Python string literal, double-quoted as the idiom is.
fn string(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

/// A float literal that always reads as one. Python spells the non-finite
/// values as strings passed to `float`.
fn render_float(x: f64) -> String {
    format!("pl.lit({})", bare_float(x))
}

/// A float as Python spells it, without the `pl.lit` wrapper. The non-finite
/// values have no numeric spelling and go through `float`.
fn bare_float(x: f64) -> String {
    if x.is_nan() {
        return "float(\"nan\")".to_string();
    }
    if x.is_infinite() {
        let sign = if x.is_sign_negative() { "-" } else { "" };
        return format!("float(\"{sign}inf\")");
    }
    let text = x.to_string();
    if text.contains(['.', 'e', 'E']) {
        text
    } else {
        format!("{text}.0")
    }
}

fn datetime(t: &DatetimeConst) -> String {
    let naive = match t {
        DatetimeConst::Offset(t) => t.naive_utc(),
        DatetimeConst::Naive(t) => *t,
    };
    format!(
        "pl.lit(datetime.datetime({}, {}, {}, {}, {}, {}))",
        naive.format("%Y"),
        naive.format("%-m"),
        naive.format("%-d"),
        naive.format("%-H"),
        naive.format("%-M"),
        naive.format("%-S")
    )
}

#[cfg(test)]
mod tests {
    use super::Polars;
    use crate::assert_expr::{AssertExpr, Root, check_root, lower, tests::TestEnv};
    use crate::emit::emit;

    fn py(source: &str) -> String {
        let expr = AssertExpr::parse(source).expect("parses");
        let findings = check_root(&expr, &TestEnv, Root::Any);
        assert!(findings.is_empty(), "{source:?}: {findings:?}");
        let ir = lower(&expr, &TestEnv).expect("lowers");
        emit(&Polars, &ir).expect("emits").code
    }

    fn notes(source: &str) -> Vec<&'static str> {
        let expr = AssertExpr::parse(source).expect("parses");
        let ir = lower(&expr, &TestEnv).expect("lowers");
        emit(&Polars, &ir).expect("emits").notes
    }

    /// The reason this target declares its own precedence: `&` and `|` bind
    /// tighter than comparison in Python, so every comparison beneath one has
    /// to be parenthesised or the expression means something else.
    #[test]
    fn comparison_under_a_logical_operator_is_parenthesised() {
        assert_eq!(
            py("qty > 0 AND flag"),
            r#"(pl.col("qty") > pl.lit(0)) & pl.col("flag")"#
        );
        assert_eq!(
            py("qty > 0 OR n < 1"),
            r#"(pl.col("qty") > pl.lit(0)) | (pl.col("n") < pl.lit(1))"#
        );
        // A bare conjunction of booleans needs none.
        assert_eq!(py("q3 AND q4"), r#"pl.col("q3") & pl.col("q4")"#);
        // Arithmetic binds tighter than both, so it needs none either.
        assert_eq!(py("n + 1 > 0"), r#"pl.col("n") + pl.lit(1) > pl.lit(0)"#);
    }

    #[test]
    fn a_method_call_parenthesises_a_compound_receiver() {
        assert_eq!(py("qty IS NULL"), r#"pl.col("qty").is_null()"#);
        assert_eq!(
            py("n + 1 IS NULL"),
            r#"(pl.col("n") + pl.lit(1)).is_null()"#
        );
    }

    #[test]
    fn nulls_need_no_guards_here() {
        // Each of these propagates a null in polars, as the language does, so
        // none of the guards the R target needs appears.
        assert_eq!(py("qty IS NOT NULL"), r#"pl.col("qty").is_not_null()"#);
        assert_eq!(py("IS_NAN(qty)"), r#"pl.col("qty").is_nan()"#);
        assert_eq!(py("IS_FINITE(qty)"), r#"pl.col("qty").is_finite()"#);
        // polars will not take a list of expressions, so a list of literals is
        // written plainly — which reads better anyway.
        assert_eq!(py("qty IN (1, 2)"), r#"pl.col("qty").is_in([1, 2])"#);
        assert_eq!(
            py("qty IN (n + 1, 2)"),
            r#"pl.col("qty").is_in(pl.concat_list([pl.col("n") + pl.lit(1), pl.lit(2)]))"#
        );
        assert_eq!(py("COUNT(s) > 0"), r#"pl.col("s").count() > pl.lit(0)"#);
    }

    #[test]
    fn the_two_guarded_mappings_carry_their_guard() {
        // `n_unique` counts a null as a distinct value, so they are dropped.
        assert_eq!(
            py("COUNT_DISTINCT(s) <= 16"),
            r#"pl.col("s").drop_nulls().n_unique() <= pl.lit(16)"#
        );
        // A date plus a duration stays a date, so it is promoted first.
        assert_eq!(
            py("d + interval(12, hours) < NOW()"),
            r#"pl.col("d").cast(pl.Datetime("us")) + pl.duration(hours=pl.lit(12)) < pl.lit(datetime.datetime.now())"#
        );
    }

    #[test]
    fn patterns_use_the_clearest_native_form() {
        assert_eq!(py("s LIKE 'NZ-%'"), r#"pl.col("s").str.starts_with("NZ-")"#);
        assert_eq!(py("s LIKE '%.nz'"), r#"pl.col("s").str.ends_with(".nz")"#);
        assert_eq!(py("s LIKE 'exact'"), r#"pl.col("s") == pl.lit("exact")"#);
        assert_eq!(py("s LIKE 'a%b'"), r#"pl.col("s").str.contains("^a.*b$")"#);
        assert_eq!(
            py("s SIMILAR TO '[a-z]+'"),
            r#"pl.col("s").str.contains("^(?:[a-z]+)$")"#
        );
        assert_eq!(
            py("s NOT LIKE 'NZ-%'"),
            r#"~pl.col("s").str.starts_with("NZ-")"#
        );
    }

    #[test]
    fn a_selection_stays_a_selection() {
        // `all_horizontal` is an ordinary expression with the language's own
        // combination and null semantics, so it need not be expanded.
        assert_eq!(
            py("COLUMNS('q[34]') IS NOT NULL"),
            r#"pl.all_horizontal(pl.col("^.*(?:q[34]).*$").is_not_null())"#
        );
        assert_eq!(
            py("COLUMNS(*) IS NOT NULL"),
            r#"pl.all_horizontal(pl.all().is_not_null())"#
        );
        assert_eq!(
            py("COLUMNS([q3, q4]) IS NOT NULL"),
            r#"pl.all_horizontal(pl.col("q3", "q4").is_not_null())"#
        );
    }

    #[test]
    fn a_case_chains_when_and_then() {
        assert_eq!(
            py("CASE WHEN flag THEN 1 ELSE 2 END > 0"),
            r#"pl.when(pl.col("flag")).then(pl.lit(1)).otherwise(pl.lit(2)) > pl.lit(0)"#
        );
        // Without `otherwise` polars gives null, which is the ELSE-less form.
        assert_eq!(
            py("CASE WHEN flag THEN 1 END > 0"),
            r#"pl.when(pl.col("flag")).then(pl.lit(1)) > pl.lit(0)"#
        );
    }

    #[test]
    fn divergences_attach_a_note_by_being_used() {
        assert!(notes("flag").is_empty());
        assert!(notes("s = 'a'").is_empty());
        // polars keeps null and NaN apart, so `IS NULL` is exact...
        assert!(notes("s IS NULL").is_empty());
        // ...but a comparison a NaN can reach is not.
        assert!(notes("qty > 0")[0].contains("NaN"));
        assert!(notes("ROUND(qty) > 0").iter().any(|n| n.contains("halves")));
        assert!(notes("SUM(qty) > 0").iter().any(|n| n.contains("identity")));
        assert!(notes("ANY(flag)").iter().any(|n| n.contains("identity")));
        // `mean`, `min` and `max` return null on an empty column, as the
        // language does, so they carry no empty-fold note.
        assert!(!notes("AVG(qty) > 0").iter().any(|n| n.contains("identity")));
        assert!(notes("n + 1 > 0").iter().any(|n| n.contains("wraps")));
        // `%` takes its sign from the divisor here, so it needs no note.
        assert!(!notes("MOD(n, 3) = 0").iter().any(|n| n.contains("modulus")));
    }
}
