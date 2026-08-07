//! `SQL(duckdb)`.
//!
//! The closest target to the language, and the first one built: DuckDB shares
//! the language's three-valued logic, its RE2 regexes, its float division, and
//! its half-away-from-zero rounding. Two things differ, both about arithmetic
//! that [the language reports rather than answering](https://data-dict.tidyverse.org/expression-execution.html#no-result),
//! and neither can be guarded in an expression.

use super::{Ctx, Fidelity, Side, Target, Unsupported, prec};
use crate::assert_expr::{
    ArithOp, CmpOp, DatetimeConst, IntervalUnit, LikePattern, NodeKind, Op, TypedExpr,
};

pub struct DuckDb;

impl Target for DuckDb {
    fn name(&self) -> &'static str {
        "SQL(duckdb)"
    }

    fn prec(&self, e: &TypedExpr) -> u8 {
        match &e.kind {
            NodeKind::Or(..) => prec::OR,
            NodeKind::And(..) => prec::AND,
            NodeKind::Not(_) => prec::NOT,
            NodeKind::Compare { .. }
            | NodeKind::IsNull { .. }
            | NodeKind::Between { .. }
            | NodeKind::In { .. }
            | NodeKind::Like { .. } => prec::CMP,
            NodeKind::Arith { op, .. } => match op {
                ArithOp::Add | ArithOp::Sub => prec::ADD,
                ArithOp::Mul | ArithOp::Div => prec::MUL,
            },
            NodeKind::Neg(_) => prec::NEG,
            // `SIMILAR TO` becomes a function call, so it needs no wrapping.
            _ => prec::ATOM,
        }
    }

    fn column(&self, path: &[String]) -> String {
        path.iter()
            .map(|segment| format!("\"{}\"", segment.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(".")
    }

    fn conjunction(&self) -> (&'static str, u8) {
        ("AND", prec::AND)
    }

    fn write(&self, cx: &mut Ctx, e: &TypedExpr) -> Result<(), Unsupported> {
        match &e.kind {
            NodeKind::Int(n) => cx.push(&n.to_string()),
            NodeKind::Float(x) => cx.push(&render_float(*x)),
            NodeKind::Str(s) => cx.push(&quote(s)),
            NodeKind::Bool(b) => cx.push(if *b { "TRUE" } else { "FALSE" }),
            NodeKind::Null => cx.push("NULL"),
            NodeKind::Date(d) => cx.push(&format!("DATE '{d}'")),
            NodeKind::Datetime(t) => cx.push(&format!("TIMESTAMP '{}'", datetime(t))),
            NodeKind::Now => cx.push("current_timestamp"),
            NodeKind::Column(c) => cx.push(&self.column(&c.path)),
            NodeKind::Selected => {
                let column = cx
                    .selected()
                    .expect("a selection is expanded one column at a time");
                let text = self.column(&column.path);
                cx.push(&text);
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
                // A zero divisor is `inf` here where the language reports it,
                // and no expression can raise, so this is declared not guarded.
                if *op == ArithOp::Div {
                    cx.fidelity(DIVISION);
                }
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
                    CmpOp::Ne => "<>",
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
                operand,
                list,
                negated,
            } => {
                cx.child(prec::CMP, Side::Left, operand)?;
                cx.push(if *negated { " NOT IN (" } else { " IN (" });
                for (i, item) in list.iter().enumerate() {
                    if i > 0 {
                        cx.push(", ");
                    }
                    cx.free(item)?;
                }
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
                // RE2 both sides, anchored both sides: exact by construction.
                if *negated {
                    cx.push("NOT ");
                }
                cx.call("regexp_full_match", &[operand, pattern])?;
            }
            NodeKind::Interval { n, unit } => write_interval(cx, n, *unit)?,
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
            NodeKind::Func { op, args } => write_func(cx, *op, args)?,
        }
        Ok(())
    }
}

/// `/` yields infinity here where the language reports a zero divisor. There is
/// no portable expression that raises, so this is declared rather than guarded.
const DIVISION: Fidelity = Fidelity::Divergent(
    "DuckDB yields infinity when dividing by zero, where data-dict reports it (D10).",
);

/// `MOD` by zero is null here, and reported by data-dict, for the same reason.
const MODULO: Fidelity =
    Fidelity::Divergent("DuckDB yields null for a zero modulus, where data-dict reports it (D10).");

/// `SUM` widens to 128 bits here, so a total data-dict reports as an overflow
/// can succeed.
const SUM: Fidelity = Fidelity::Divergent(
    "DuckDB sums integers at 128 bits, so a total data-dict reports as an overflow (D09) may succeed.",
);

fn write_like(
    cx: &mut Ctx,
    operand: &TypedExpr,
    pattern: &LikePattern,
    negated: bool,
) -> Result<(), Unsupported> {
    // The decomposed forms read better than a regex and mean the same thing.
    match pattern {
        LikePattern::Exact(text) => {
            cx.child(prec::CMP, Side::Left, operand)?;
            cx.push(if negated { " <> " } else { " = " });
            cx.push(&quote(text));
        }
        LikePattern::Prefix(text) | LikePattern::Suffix(text) => {
            let name = if matches!(pattern, LikePattern::Prefix(_)) {
                "starts_with"
            } else {
                "ends_with"
            };
            if negated {
                cx.push("NOT ");
            }
            cx.push(name);
            cx.push("(");
            cx.free(operand)?;
            cx.push(", ");
            cx.push(&quote(text));
            cx.push(")");
        }
        LikePattern::Regex(re) => {
            if negated {
                cx.push("NOT ");
            }
            cx.push("regexp_full_match(");
            cx.free(operand)?;
            cx.push(", ");
            cx.push(&quote(re));
            cx.push(")");
        }
        // DuckDB's own `LIKE` takes a computed pattern, so the one case other
        // targets have to refuse works here unchanged.
        LikePattern::Dynamic(p) => {
            cx.child(prec::CMP, Side::Left, operand)?;
            cx.push(if negated { " NOT LIKE " } else { " LIKE " });
            cx.child(prec::CMP, Side::Right, p)?;
        }
    }
    Ok(())
}

fn write_interval(cx: &mut Ctx, n: &TypedExpr, unit: IntervalUnit) -> Result<(), Unsupported> {
    let unit = match unit {
        IntervalUnit::Seconds => "seconds",
        IntervalUnit::Minutes => "minutes",
        IntervalUnit::Hours => "hours",
        IntervalUnit::Days => "days",
        IntervalUnit::Weeks => "weeks",
    };
    // A literal count folds into the interval literal, which is the readable
    // form; anything computed has to multiply a unit interval.
    if let NodeKind::Int(count) = &n.kind {
        cx.push(&format!("INTERVAL '{count} {unit}'"));
        return Ok(());
    }
    cx.push("(");
    cx.free(n)?;
    cx.push(&format!(" * INTERVAL '1 {unit}')"));
    Ok(())
}

fn write_func(cx: &mut Ctx, op: Op, args: &[TypedExpr]) -> Result<(), Unsupported> {
    let refs: Vec<&TypedExpr> = args.iter().collect();
    let simple = |cx: &mut Ctx, name: &str| cx.call(name, &refs);
    match op {
        Op::Length => simple(cx, "length")?,
        Op::Lower => simple(cx, "lower")?,
        Op::Upper => simple(cx, "upper")?,
        Op::Trim => simple(cx, "trim")?,
        Op::StartsWith => simple(cx, "starts_with")?,
        Op::EndsWith => simple(cx, "ends_with")?,
        Op::Abs => simple(cx, "abs")?,
        Op::Floor => simple(cx, "floor")?,
        Op::Ceil => simple(cx, "ceil")?,
        // DuckDB rounds halves away from zero, as the language does.
        Op::Round => simple(cx, "round")?,
        Op::Mod => {
            cx.fidelity(MODULO);
            simple(cx, "mod")?;
        }
        Op::Min => simple(cx, "min")?,
        Op::Max => simple(cx, "max")?,
        Op::Sum => {
            cx.fidelity(SUM);
            simple(cx, "sum")?;
        }
        Op::Avg => simple(cx, "avg")?,
        Op::Count => simple(cx, "count")?,
        Op::RowCount => cx.push("count(*)"),
        Op::CountDistinct => {
            cx.push("count(DISTINCT ");
            cx.free(&args[0])?;
            cx.push(")");
        }
        // Null on all-null input, exactly as the language specifies.
        Op::Any => simple(cx, "bool_or")?,
        Op::All => simple(cx, "bool_and")?,
    }
    Ok(())
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// A float that always reads as one, so `2.0` doesn't become an integer.
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
