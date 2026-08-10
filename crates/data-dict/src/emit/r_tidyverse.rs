//! `R(tidyverse)` — dplyr and stringr.
//!
//! R's `&`, `|` and `!` are already three-valued over `NA`, so the logic needs
//! nothing. Three things do need work: `%in%` answers `FALSE` for an `NA`
//! subject where the language says null, `%%` takes its sign from the divisor
//! where the language takes it from the dividend, and adding a duration to a
//! `Date` keeps it a `Date` where the language produces a datetime. Each is
//! guarded, so those three are exact rather than approximate.

use super::{Ctx, Fidelity, Side, Target, Unsupported};
use crate::assert_expr::{
    ArithOp, CmpOp, DatetimeConst, IntervalUnit, LikePattern, NodeKind, Op, Selection,
    SelectorForm, Type, TypedExpr,
};

pub struct RTidyverse;

/// R's precedence, which differs from SQL's in one place that matters: an
/// infix `%…%` operator binds *tighter* than `*` and `/`, so `%in%` has a level
/// of its own above them.
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
            // A guarded `IN` is an `|`, and a guarded `MOD` is a subtraction:
            // both sit where their emitted form sits, not where the language's
            // operator would.
            NodeKind::In { .. } => p::OR,
            NodeKind::Func { op: Op::Mod, .. } => p::ADD,
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
                if *op == ArithOp::Div {
                    cx.fidelity(DIVISION);
                }
                if is_shift(lhs, rhs) {
                    return write_shift(self, cx, *op, lhs, rhs);
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
            NodeKind::Func { op, args } => write_func(cx, *op, args)?,
        }
        Ok(())
    }
}

const DIVISION: Fidelity =
    Fidelity::Divergent("R yields Inf when dividing by zero, where data-dict reports it (D10).");

const MODULO_ZERO: Fidelity =
    Fidelity::Divergent("R yields NaN for a zero modulus, where data-dict reports it (D10).");

const ROUNDING: Fidelity = Fidelity::Divergent(
    "R rounds halves to even, where data-dict rounds them away from zero, so results differ on an exact half.",
);

const REGEX: Fidelity = Fidelity::Divergent(
    "stringr matches with ICU regular expressions, where data-dict uses RE2; the syntaxes differ in corners.",
);

const EMPTY_FOLD: Fidelity = Fidelity::Divergent(
    "R folds an empty or all-null column to the identity (0, FALSE, TRUE, Inf) where data-dict returns null, so an aggregate assertion differs on such a column.",
);

const OVERFLOW: Fidelity = Fidelity::Divergent(
    "R has no 64-bit integers, so arithmetic data-dict reports as an overflow (D09) yields a double here.",
);

/// Whether this arithmetic is a temporal shift, which needs the operand
/// promoted before the duration is added.
fn is_shift(lhs: &TypedExpr, rhs: &TypedExpr) -> bool {
    lhs.ty == Type::Interval || rhs.ty == Type::Interval
}

/// A `Date` plus a duration stays a `Date` in R, silently dropping anything
/// shorter than a day; the language makes it a datetime. Promoting the date
/// first restores that, so the mapping is exact rather than approximate.
fn write_shift(
    target: &RTidyverse,
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
    let _ = target;
    Ok(())
}

/// `%in%` answers `FALSE` for an `NA` subject where the language says null, and
/// null passes. The guard restores that.
fn write_in(
    cx: &mut Ctx,
    operand: &TypedExpr,
    list: &[TypedExpr],
    negated: bool,
) -> Result<(), Unsupported> {
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

fn write_func(cx: &mut Ctx, op: Op, args: &[TypedExpr]) -> Result<(), Unsupported> {
    let refs: Vec<&TypedExpr> = args.iter().collect();
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
        // R's `%%` takes its sign from the divisor; the language takes it from
        // the dividend, which this arithmetic restores.
        Op::Mod => {
            cx.fidelity(MODULO_ZERO);
            let (x, y) = (&args[0], &args[1]);
            cx.child(p::ADD, Side::Left, x)?;
            cx.push(" - ");
            cx.child(p::MUL, Side::Right, y)?;
            // The division inside `trunc` is an operator position, not a
            // delimited one: a compound dividend has to keep its brackets or
            // `/` steals its last term.
            cx.push(" * trunc(");
            cx.child(p::MUL, Side::Left, x)?;
            cx.push(" / ");
            cx.child(p::MUL, Side::Right, y)?;
            cx.push(")");
        }
        Op::Min | Op::Max | Op::Sum | Op::Avg => {
            cx.fidelity(EMPTY_FOLD);
            if op == Op::Sum {
                cx.fidelity(OVERFLOW);
            }
            let name = match op {
                Op::Min => "min",
                Op::Max => "max",
                Op::Sum => "sum",
                _ => "mean",
            };
            cx.push(name);
            cx.push("(");
            cx.free(&args[0])?;
            cx.push(", na.rm = TRUE)");
        }
        Op::Any | Op::All => {
            cx.fidelity(EMPTY_FOLD);
            cx.push(if op == Op::Any { "any(" } else { "all(" });
            cx.free(&args[0])?;
            cx.push(", na.rm = TRUE)");
        }
        Op::Count => {
            cx.push("sum(!is.na(");
            cx.free(&args[0])?;
            cx.push("))");
        }
        Op::RowCount => cx.push("n()"),
        Op::CountDistinct => {
            cx.push("n_distinct(");
            cx.free(&args[0])?;
            cx.push(", na.rm = TRUE)");
        }
    }
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
    let text = x.to_string();
    if text.contains(['.', 'e', 'E', 'n', 'i']) {
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
