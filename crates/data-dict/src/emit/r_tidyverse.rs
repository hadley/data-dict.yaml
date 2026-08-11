//! `R(tidyverse)` — dplyr and stringr.
//!
//! R's `&`, `|` and `!` are already three-valued over `NA`, so the logic needs
//! nothing, and `%%` agrees with the language about a remainder's sign. Three
//! things do need work: `%in%` answers `FALSE` for an `NA` subject where the
//! language says null, adding a duration to a `Date` keeps it a `Date` where the
//! language produces a datetime, and `is.nan`/`is.finite` answer `FALSE` for an
//! `NA` where a null argument must stay null. All three are guarded, so they are
//! exact rather than approximate.
//!
//! What can't be guarded is that R reads a NaN as *missing*: comparing against
//! one gives `NA`, `is.na` is `TRUE`, and `na.rm` drops it, where
//! [the language treats it as a value](https://data-dict.tidyverse.org/floating-point.html#non-finite).
//! Each of those travels as a note.

use super::{Ctx, Fidelity, Side, Target, Unsupported};
use crate::assert_expr::{
    ArithOp, CmpOp, DatetimeConst, IntervalUnit, LikePattern, NodeKind, Op, Selection,
    SelectorForm, Type, TypedExpr,
};

pub struct RTidyverse;

/// R's precedence, which differs from SQL's in one place that matters: an
/// infix `%…%` operator binds *tighter* than `*` and `/`, so `%in%` and `%%`
/// have a level of their own above them.
mod p {
    pub const OR: u8 = 1;
    pub const AND: u8 = 2;
    pub const NOT: u8 = 3;
    pub const CMP: u8 = 4;
    pub const ADD: u8 = 5;
    pub const MUL: u8 = 6;
    pub const SPECIAL: u8 = 7;
    pub const NEG: u8 = 8;
    pub const ATOM: u8 = 9;
}

impl Target for RTidyverse {
    fn name(&self) -> &'static str {
        "R(tidyverse)"
    }

    fn prec(&self, e: &TypedExpr) -> u8 {
        match &e.kind {
            NodeKind::Or(..) => p::OR,
            // A guarded `IN` is an `|`, and `MOD` is `%%`: both sit where their
            // emitted form sits, not where the language's operator would.
            NodeKind::In { .. } => p::OR,
            NodeKind::Func { op: Op::Mod, .. } => p::SPECIAL,
            NodeKind::And(..) => p::AND,
            NodeKind::Not(_) => p::NOT,
            NodeKind::Compare { .. } | NodeKind::Between { .. } => p::CMP,
            NodeKind::Arith { op, .. } => match op {
                ArithOp::Add | ArithOp::Sub => p::ADD,
                ArithOp::Mul | ArithOp::Div => p::MUL,
            },
            NodeKind::Neg(_) => p::NEG,
            _ => p::ATOM,
        }
    }

    fn column(&self, path: &[String]) -> String {
        // A struct column is a data-frame column of its own, reached with `$`.
        path.iter()
            .map(|segment| name(segment))
            .collect::<Vec<_>>()
            .join("$")
    }

    fn conjunction(&self) -> (&'static str, u8) {
        ("&", p::AND)
    }

    fn write_selection(
        &self,
        cx: &mut Ctx,
        selection: &Selection,
        root: &TypedExpr,
    ) -> Result<bool, Unsupported> {
        // `if_all` is an ordinary value with the spec's combination and null
        // semantics, so the selection stays a selection instead of being
        // written out column by column.
        cx.push("if_all(");
        match &selection.form {
            SelectorForm::All => cx.push("everything()"),
            // `matches()` is unanchored, like the language's own regex.
            SelectorForm::Regex(pattern) => cx.push(&format!("matches({})", string(pattern))),
            SelectorForm::List => {
                cx.push("c(");
                cx.comma_separated(&selection.columns, |cx, column| {
                    cx.push(&self.column(&column.path));
                    Ok(())
                })?;
                cx.push(")");
            }
        }
        cx.push(", \\(x) ");
        cx.with_selected("x".to_string(), |cx| cx.free(root))?;
        cx.push(")");
        Ok(true)
    }

    fn write(&self, cx: &mut Ctx, e: &TypedExpr) -> Result<(), Unsupported> {
        match &e.kind {
            NodeKind::Int(n) => cx.push(&format!("{n}L")),
            NodeKind::Float(x) => cx.push(&render_float(*x)),
            NodeKind::Str(s) => cx.push(&string(s)),
            NodeKind::Bool(b) => cx.push(if *b { "TRUE" } else { "FALSE" }),
            NodeKind::Null => cx.push("NA"),
            NodeKind::Date(d) => cx.push(&format!("as.Date(\"{d}\")")),
            NodeKind::Datetime(t) => {
                cx.push(&format!("as.POSIXct(\"{}\", tz = \"UTC\")", datetime(t)));
            }
            NodeKind::Now => cx.push("Sys.time()"),
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
                cx.push("!");
                cx.child(p::NOT, Side::Right, x)?;
            }
            NodeKind::And(l, r) => cx.infix(p::AND, "&", l, r)?,
            NodeKind::Or(l, r) => cx.infix(p::OR, "|", l, r)?,
            NodeKind::Arith { op, lhs, rhs } => {
                if is_interval_shift(lhs, rhs) {
                    return write_interval_shift(cx, *op, lhs, rhs);
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
            NodeKind::IsNull { operand, negated } => {
                if super::over_numbers(&[operand]) {
                    cx.fidelity(NAN_IS_MISSING);
                }
                if *negated {
                    cx.push("!");
                }
                cx.call("is.na", &[operand])?;
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
                    cx.push("!");
                }
                cx.push("between(");
                cx.free(operand)?;
                cx.push(", ");
                cx.free(lo)?;
                cx.push(", ");
                cx.free(hi)?;
                cx.push(")");
            }
            NodeKind::In {
                needle,
                haystack,
                negated,
            } => write_in(cx, needle, haystack, *negated)?,
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
                cx.fidelity(REGEX);
                if *negated {
                    cx.push("!");
                }
                cx.push("str_detect(");
                cx.free(operand)?;
                cx.push(", ");
                anchored(cx, pattern)?;
                cx.push(")");
            }
            NodeKind::Interval { n, unit } => {
                cx.push("as.difftime(");
                cx.free(n)?;
                cx.push(&format!(", units = \"{}\")", units(*unit)));
            }
            NodeKind::Case { whens, els } => {
                cx.push("case_when(");
                for (cond, result) in whens {
                    cx.free(cond)?;
                    cx.push(" ~ ");
                    cx.free(result)?;
                    cx.push(", ");
                }
                match els {
                    Some(els) => {
                        cx.push(".default = ");
                        cx.free(els)?;
                    }
                    // Unmatched rows are NA by default, which is what the
                    // language says an `ELSE`-less `CASE` gives.
                    None => cx.push(".default = NA"),
                }
                cx.push(")");
            }
            NodeKind::Func { op, args, filter } => {
                write_func(cx, *op, args, filter.as_deref())?;
            }
        }
        Ok(())
    }
}

const NAN_COMPARISON: Fidelity = Fidelity::Divergent(
    "R reads a NaN as missing, so comparing against one gives NA where data-dict answers false; a row holding one passes here and is reported there.",
);

const NAN_IS_MISSING: Fidelity = Fidelity::Divergent(
    "`is.na` is TRUE for a NaN, where data-dict counts a NaN as a value and answers false for `IS NULL`.",
);

const NAN_DROPPED: Fidelity = Fidelity::Divergent(
    "`na.rm = TRUE` drops a NaN along with the nulls, where data-dict folds it in — so an aggregate over a column holding one differs.",
);

/// R's doubles agree with the language (`7 %% 0` is `NaN`), but its integers
/// don't, and a column read as `integer` takes that path.
const MODULO_ZERO: Fidelity = Fidelity::Divergent(
    "R yields NA for an integer modulus by zero (`7L %% 0L`), where data-dict yields a NaN.",
);

const ROUNDING: Fidelity = Fidelity::Divergent(
    "R rounds halves to even, where data-dict rounds them away from zero, so results differ on an exact half.",
);

const REGEX: Fidelity = Fidelity::Divergent(
    "stringr matches with ICU regular expressions, where data-dict uses RE2; the syntaxes differ in corners.",
);

const EMPTY_FOLD: Fidelity = Fidelity::Divergent(
    "R folds an empty or all-null column to the identity (0, FALSE, TRUE, Inf) where data-dict returns null — and data-dict gives an infinity only for a column that really holds one — so an aggregate assertion differs on such a column.",
);

const OVERFLOW: Fidelity = Fidelity::Divergent(
    "R has no 64-bit integers, so arithmetic data-dict reports as an overflow (D09) yields a double here.",
);

/// Whether this arithmetic shifts a date or datetime by an interval, which
/// needs the shifted operand promoted before the duration is added.
fn is_interval_shift(lhs: &TypedExpr, rhs: &TypedExpr) -> bool {
    lhs.ty == Type::Interval || rhs.ty == Type::Interval
}

/// A `Date` plus a duration stays a `Date` in R, silently dropping anything
/// shorter than a day; the language makes it a datetime. Promoting the date
/// first restores that, so the mapping is exact rather than approximate.
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
        cx.push("as.POSIXct(");
        cx.free(base)?;
        cx.push(", tz = \"UTC\")");
    } else {
        cx.child(p::ADD, Side::Left, base)?;
    }
    cx.push(if op == ArithOp::Sub { " - " } else { " + " });
    cx.child(p::ADD, Side::Right, duration)?;
    Ok(())
}

/// `%in%` answers `FALSE` for an `NA` subject where the language says null, and
/// null passes. The guard restores that — but it catches a NaN with it, since
/// `is.na` is `TRUE` for one, which is why a numeric subject diverges.
fn write_in(
    cx: &mut Ctx,
    operand: &TypedExpr,
    list: &[TypedExpr],
    negated: bool,
) -> Result<(), Unsupported> {
    if super::over_numbers(&[operand]) {
        cx.fidelity(NAN_COMPARISON);
    }
    cx.push("is.na(");
    cx.free(operand)?;
    cx.push(") | ");
    if negated {
        cx.push("!");
    }
    cx.push("(");
    cx.child(p::SPECIAL, Side::Left, operand)?;
    cx.push(" %in% c(");
    cx.comma_separated(list, |cx, item| cx.free(item))?;
    cx.push("))");
    Ok(())
}

fn write_like(
    cx: &mut Ctx,
    operand: &TypedExpr,
    pattern: &LikePattern,
    negated: bool,
) -> Result<(), Unsupported> {
    match pattern {
        LikePattern::Exact(text) => {
            cx.child(p::CMP, Side::Left, operand)?;
            cx.push(if negated { " != " } else { " == " });
            cx.push(&string(text));
        }
        LikePattern::Prefix(text) | LikePattern::Suffix(text) => {
            let name = if matches!(pattern, LikePattern::Prefix(_)) {
                "str_starts"
            } else {
                "str_ends"
            };
            if negated {
                cx.push("!");
            }
            cx.push(name);
            cx.push("(");
            cx.free(operand)?;
            // `fixed()` matters: without it the pattern is a regex, so a `.` in
            // a `LIKE` pattern would stop being a literal dot.
            cx.push(&format!(", fixed({}))", string(text)));
        }
        LikePattern::Regex(re) => {
            cx.fidelity(REGEX);
            if negated {
                cx.push("!");
            }
            cx.push("str_detect(");
            cx.free(operand)?;
            cx.push(&format!(", {})", string(re)));
        }
        // A `LIKE` pattern is decomposed when it is a literal. A computed one
        // would have to be turned into a regex at run time, and R has nothing
        // that does it.
        LikePattern::Dynamic(_) => {
            return Err(Unsupported {
                what: "`LIKE` with a computed pattern",
                why: "the pattern must be a literal so its wildcards can be translated; \
                      R has no run-time equivalent",
            });
        }
    }
    Ok(())
}

fn write_func(
    cx: &mut Ctx,
    op: Op,
    args: &[TypedExpr],
    filter: Option<&TypedExpr>,
) -> Result<(), Unsupported> {
    let refs: Vec<&TypedExpr> = args.iter().collect();
    // A filtered aggregate folds a subset of rows, so subset the argument by
    // the condition. An NA in the condition subsets to an NA element, which
    // the aggregate's own `na.rm` then drops — the row is excluded either way.
    let folded = |cx: &mut Ctx, arg: &TypedExpr| -> Result<(), Unsupported> {
        cx.child(p::ATOM, Side::Left, arg)?;
        if let Some(filter) = filter {
            cx.push("[");
            cx.free(filter)?;
            cx.push("]");
        }
        Ok(())
    };
    match op {
        Op::Length => cx.call("str_length", &refs)?,
        Op::Lower => cx.call("str_to_lower", &refs)?,
        Op::Upper => cx.call("str_to_upper", &refs)?,
        Op::Trim => cx.call("str_trim", &refs)?,
        Op::StartsWith | Op::EndsWith => {
            let name = if op == Op::StartsWith {
                "str_starts"
            } else {
                "str_ends"
            };
            cx.push(name);
            cx.push("(");
            cx.free(&args[0])?;
            cx.push(", fixed(");
            cx.free(&args[1])?;
            cx.push("))");
        }
        Op::Abs => cx.call("abs", &refs)?,
        Op::Floor => cx.call("floor", &refs)?,
        Op::Ceil => cx.call("ceiling", &refs)?,
        Op::Round => {
            cx.fidelity(ROUNDING);
            cx.call("round", &refs)?;
        }
        Op::Mod => {
            cx.fidelity(MODULO_ZERO);
            cx.infix(p::SPECIAL, "%%", &args[0], &args[1])?;
        }
        Op::IsFinite => write_kind_test(cx, "is.finite", &args[0])?,
        Op::IsInfinite => write_kind_test(cx, "is.infinite", &args[0])?,
        Op::IsNan => write_kind_test(cx, "is.nan", &args[0])?,
        Op::Min | Op::Max | Op::Sum | Op::Avg => {
            cx.fidelity(EMPTY_FOLD);
            if op == Op::Sum {
                cx.fidelity(OVERFLOW);
            }
            if super::over_numbers(&[&args[0]]) {
                cx.fidelity(NAN_DROPPED);
            }
            let name = match op {
                Op::Min => "min",
                Op::Max => "max",
                Op::Sum => "sum",
                _ => "mean",
            };
            cx.push(name);
            cx.push("(");
            folded(cx, &args[0])?;
            cx.push(", na.rm = TRUE)");
        }
        Op::Any | Op::All => {
            cx.fidelity(EMPTY_FOLD);
            cx.push(if op == Op::Any { "any(" } else { "all(" });
            folded(cx, &args[0])?;
            cx.push(", na.rm = TRUE)");
        }
        Op::Count => {
            if super::over_numbers(&[&args[0]]) {
                cx.fidelity(NAN_DROPPED);
            }
            cx.push("sum(!is.na(");
            folded(cx, &args[0])?;
            cx.push("))");
        }
        Op::RowCount => match filter {
            // `n()` takes no subset, so count the rows the condition keeps.
            Some(filter) => {
                cx.push("sum(");
                cx.free(filter)?;
                cx.push(", na.rm = TRUE)");
            }
            None => cx.push("n()"),
        },
        Op::CountDistinct => {
            if super::over_numbers(&[&args[0]]) {
                cx.fidelity(NAN_DROPPED);
            }
            cx.push("n_distinct(");
            folded(cx, &args[0])?;
            cx.push(", na.rm = TRUE)");
        }
    }
    Ok(())
}

/// `is.nan` and its siblings answer `FALSE` for an `NA`, where a scalar function
/// of the language propagates a null. `is.na(x) & !is.nan(x)` is R's test for a
/// missing value that isn't a NaN, which is exactly where the two disagree.
fn write_kind_test(cx: &mut Ctx, test: &str, x: &TypedExpr) -> Result<(), Unsupported> {
    cx.push("if_else(is.na(");
    cx.free(x)?;
    cx.push(") & !is.nan(");
    cx.free(x)?;
    cx.push("), NA, ");
    cx.call(test, &[x])?;
    cx.push(")");
    Ok(())
}

fn anchored(cx: &mut Ctx, pattern: &TypedExpr) -> Result<(), Unsupported> {
    // `SIMILAR TO` matches the whole string; `str_detect` matches anywhere.
    if let NodeKind::Str(text) = &pattern.kind {
        cx.push(&string(&format!("^(?:{text})$")));
        return Ok(());
    }
    cx.push("paste0(\"^(?:\", ");
    cx.free(pattern)?;
    cx.push(", \")$\")");
    Ok(())
}

fn units(unit: IntervalUnit) -> &'static str {
    match unit {
        IntervalUnit::Seconds => "secs",
        IntervalUnit::Minutes => "mins",
        IntervalUnit::Hours => "hours",
        IntervalUnit::Days => "days",
        IntervalUnit::Weeks => "weeks",
    }
}

/// A column name, backtick-quoted when it isn't a syntactic R name.
fn name(text: &str) -> String {
    let syntactic = !text.is_empty()
        && text
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '.')
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_');
    if syntactic {
        text.to_string()
    } else {
        format!("`{}`", text.replace('`', "\\`"))
    }
}

fn string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn render_float(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x.is_sign_negative() { "-Inf" } else { "Inf" }.to_string();
    }
    let text = x.to_string();
    if text.contains(['.', 'e', 'E']) {
        text
    } else {
        format!("{text}.0")
    }
}

fn datetime(t: &DatetimeConst) -> String {
    match t {
        DatetimeConst::Offset(t) => t.naive_utc().to_string(),
        DatetimeConst::Naive(t) => t.to_string(),
    }
}
