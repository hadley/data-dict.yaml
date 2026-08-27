//! `R(base)`, `R(tidyverse)` and `R(data.table)` — one emitter, three dialects.
//!
//! All three share R's semantics, so most of the mapping is shared: `&`, `|`
//! and `!` are already three-valued over `NA`, so the logic needs nothing, and
//! `%%` agrees with the language about a remainder's sign. Three things need
//! work everywhere: `%in%` answers `FALSE` for an `NA` subject where the
//! language says null, adding a duration to a `Date` keeps it a `Date` where
//! the language produces a datetime, and `is.nan`/`is.finite` answer `FALSE`
//! for an `NA` where a null argument must stay null. All three are guarded, so
//! they are exact rather than approximate.
//!
//! The dialects differ in spelling. The tidyverse has stringr's `str_*`
//! functions, `if_all` for a selection, `case_when`, and `n()`/`n_distinct`
//! for the aggregates. Base R and data.table spell strings with `nchar`,
//! `startsWith` and `grepl` — `grepl` answers `FALSE` for an `NA` where
//! stringr propagates it, so its matches carry a null guard — and expand a
//! selection to a conjunction. data.table adds `.N`, `uniqueN`, `fifelse` and
//! `fcase`; base R has no row count a bare predicate can name, so it refuses
//! `ROW_COUNT()`.
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

/// Which R idiom the output is spelled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Base,
    Tidyverse,
    DataTable,
}

#[derive(Debug, Clone, Copy)]
pub struct R(Dialect);

pub const R_BASE: R = R(Dialect::Base);
pub const R_TIDYVERSE: R = R(Dialect::Tidyverse);
pub const R_DATA_TABLE: R = R(Dialect::DataTable);

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

impl R {
    /// The two-argument conditional: dplyr's `if_else`, base R's `ifelse`,
    /// data.table's `fifelse`.
    fn conditional(&self) -> &'static str {
        match self.0 {
            Dialect::Base => "ifelse",
            Dialect::Tidyverse => "if_else",
            Dialect::DataTable => "fifelse",
        }
    }
}

impl Target for R {
    fn name(&self) -> &'static str {
        match self.0 {
            Dialect::Base => "R(base)",
            Dialect::Tidyverse => "R(tidyverse)",
            Dialect::DataTable => "R(data.table)",
        }
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
            // Base R expands `BETWEEN` to a conjunction of comparisons.
            NodeKind::Between { .. } if self.0 == Dialect::Base => p::AND,
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
        // Only the tidyverse has a self-contained idiom: `if_all` is an
        // ordinary value with the spec's combination and null semantics.
        // Base R and data.table expand the selection to a conjunction.
        if self.0 != Dialect::Tidyverse {
            return Ok(false);
        }
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
                if self.0 == Dialect::Base {
                    // Base R has no `between`; `x >= lo & x <= hi` propagates
                    // a null the same way.
                    if *negated {
                        cx.push("!(");
                    }
                    cx.child(p::CMP, Side::Left, operand)?;
                    cx.push(" >= ");
                    cx.child(p::CMP, Side::Right, lo)?;
                    cx.push(" & ");
                    cx.child(p::CMP, Side::Left, operand)?;
                    cx.push(" <= ");
                    cx.child(p::CMP, Side::Right, hi)?;
                    if *negated {
                        cx.push(")");
                    }
                } else {
                    // dplyr's and data.table's `between` are both `x >= lo &
                    // x <= hi` underneath, so a null propagates.
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
            } => write_like(self.0, cx, operand, pattern, *negated)?,
            NodeKind::SimilarTo {
                operand,
                pattern,
                negated,
            } => {
                cx.fidelity(REGEX[self.0 as usize]);
                if *negated {
                    cx.push("!");
                }
                if self.0 == Dialect::Tidyverse {
                    cx.push("str_detect(");
                    cx.free(operand)?;
                    cx.push(", ");
                    anchored(cx, pattern)?;
                    cx.push(")");
                } else {
                    // `grepl` answers FALSE for an NA where the language
                    // propagates the null, so the match is guarded.
                    write_null_guard(self.0, cx, &[operand], |cx| {
                        cx.push("grepl(");
                        anchored(cx, pattern)?;
                        cx.push(", ");
                        cx.free(operand)?;
                        cx.push(", perl = TRUE)");
                        Ok(())
                    })?;
                }
            }
            NodeKind::Interval { n, unit } => {
                cx.push("as.difftime(");
                cx.free(n)?;
                cx.push(&format!(", units = \"{}\")", units(*unit)));
            }
            NodeKind::Case { whens, els } => write_case(self.0, cx, whens, els)?,
            NodeKind::Func { op, args } => write_func(self.0, cx, *op, args)?,
        }
        Ok(())
    }
}

/// Where R and the language disagree, stated once.
///
/// Each of these is a fact about the two languages, not about a direction of
/// travel, so [reading R](crate::parse::r) attaches the same words. Keeping one
/// copy is what makes the two directions provably consistent.
pub(crate) mod notes {
    pub const NAN_COMPARISON: &str = "R reads a NaN as missing, so comparing against one gives NA where data-dict answers false; a row holding one passes here and is reported there.";

    pub const NAN_IS_MISSING: &str = "`is.na` is TRUE for a NaN, where data-dict counts a NaN as a value and answers false for `IS NULL`.";

    pub const NAN_DROPPED: &str = "`na.rm = TRUE` drops a NaN along with the nulls, where data-dict folds it in — so an aggregate over a column holding one differs.";

    /// R's doubles agree with the language (`7 %% 0` is `NaN`), but its integers
    /// don't, and a column read as `integer` takes that path.
    pub const MODULO_ZERO: &str =
        "R yields NA for an integer modulus by zero (`7L %% 0L`), where data-dict yields a NaN.";

    pub const ROUNDING: &str = "R rounds halves to even, where data-dict rounds them away from zero, so results differ on an exact half.";

    pub const REGEX_PCRE: &str =
        "R's `grepl` matches with PCRE, where data-dict uses RE2; the syntaxes differ in corners.";

    pub const REGEX_ICU: &str = "stringr matches with ICU regular expressions, where data-dict uses RE2; the syntaxes differ in corners.";

    pub const EMPTY_FOLD: &str = "R folds an empty or all-null column to the identity (0, FALSE, TRUE, Inf) where data-dict returns null — and data-dict gives an infinity only for a column that really holds one — so an aggregate assertion differs on such a column.";

    pub const OVERFLOW: &str = "R has no 64-bit integers, so arithmetic data-dict reports as an overflow (D09) yields a double here.";
}

const NAN_COMPARISON: Fidelity = Fidelity::Divergent(notes::NAN_COMPARISON);

const NAN_IS_MISSING: Fidelity = Fidelity::Divergent(notes::NAN_IS_MISSING);

const NAN_DROPPED: Fidelity = Fidelity::Divergent(notes::NAN_DROPPED);

const MODULO_ZERO: Fidelity = Fidelity::Divergent(notes::MODULO_ZERO);

const ROUNDING: Fidelity = Fidelity::Divergent(notes::ROUNDING);

/// The regex note, per dialect: stringr matches with ICU, `grepl` with PCRE.
const REGEX: [Fidelity; 3] = [
    Fidelity::Divergent(notes::REGEX_PCRE),
    Fidelity::Divergent(notes::REGEX_ICU),
    Fidelity::Divergent(notes::REGEX_PCRE),
];

const EMPTY_FOLD: Fidelity = Fidelity::Divergent(notes::EMPTY_FOLD);

const OVERFLOW: Fidelity = Fidelity::Divergent(notes::OVERFLOW);

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
    d: Dialect,
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
            let tidy = matches!(pattern, LikePattern::Prefix(_));
            if negated {
                cx.push("!");
            }
            if d == Dialect::Tidyverse {
                cx.push(if tidy { "str_starts" } else { "str_ends" });
                cx.push("(");
                cx.free(operand)?;
                // `fixed()` matters: without it the pattern is a regex, so a
                // `.` in a `LIKE` pattern would stop being a literal dot.
                cx.push(&format!(", fixed({}))", string(text)));
            } else {
                // `startsWith` propagates an NA, as the language does.
                cx.push(if tidy { "startsWith(" } else { "endsWith(" });
                cx.free(operand)?;
                cx.push(&format!(", {})", string(text)));
            }
        }
        LikePattern::Regex(re) => {
            cx.fidelity(REGEX[d as usize]);
            if negated {
                cx.push("!");
            }
            if d == Dialect::Tidyverse {
                cx.push("str_detect(");
                cx.free(operand)?;
                cx.push(&format!(", {})", string(re)));
            } else {
                write_null_guard(d, cx, &[operand], |cx| {
                    cx.push("grepl(");
                    cx.push(&string(re));
                    cx.push(", ");
                    cx.free(operand)?;
                    cx.push(", perl = TRUE)");
                    Ok(())
                })?;
            }
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

/// `ifelse(is.na(x), NA, body)` — the guard `grepl` needs, since it answers
/// `FALSE` for an `NA` subject where the language propagates the null.
fn write_null_guard(
    d: Dialect,
    cx: &mut Ctx,
    args: &[&TypedExpr],
    body: impl FnOnce(&mut Ctx) -> Result<(), Unsupported>,
) -> Result<(), Unsupported> {
    cx.push(R(d).conditional());
    cx.push("(");
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            cx.push(" | ");
        }
        cx.push("is.na(");
        cx.free(arg)?;
        cx.push(")");
    }
    cx.push(", NA, ");
    body(cx)?;
    cx.push(")");
    Ok(())
}

fn write_case(
    d: Dialect,
    cx: &mut Ctx,
    whens: &[(TypedExpr, TypedExpr)],
    els: &Option<Box<TypedExpr>>,
) -> Result<(), Unsupported> {
    match d {
        Dialect::Tidyverse => {
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
        Dialect::DataTable => {
            cx.push("fcase(");
            for (cond, result) in whens {
                cx.free(cond)?;
                cx.push(", ");
                cx.free(result)?;
                cx.push(", ");
            }
            match els {
                Some(els) => {
                    cx.push("default = ");
                    cx.free(els)?;
                }
                // `fcase` defaults to NA, as the language does.
                None => cx.push("default = NA"),
            }
            cx.push(")");
        }
        Dialect::Base => {
            // Base R has no `case_when`; nested `ifelse` says the same thing.
            let depth = whens.len();
            for (cond, result) in whens {
                cx.push("ifelse(");
                cx.free(cond)?;
                cx.push(", ");
                cx.free(result)?;
                cx.push(", ");
            }
            match els {
                Some(els) => cx.free(els)?,
                None => cx.push("NA"),
            }
            for _ in 0..depth {
                cx.push(")");
            }
        }
    }
    Ok(())
}

fn write_func(d: Dialect, cx: &mut Ctx, op: Op, args: &[TypedExpr]) -> Result<(), Unsupported> {
    let refs: Vec<&TypedExpr> = args.iter().collect();
    match op {
        Op::Length => cx.call(
            if d == Dialect::Tidyverse {
                "str_length"
            } else {
                "nchar"
            },
            &refs,
        )?,
        Op::Lower => cx.call(
            if d == Dialect::Tidyverse {
                "str_to_lower"
            } else {
                "tolower"
            },
            &refs,
        )?,
        Op::Upper => cx.call(
            if d == Dialect::Tidyverse {
                "str_to_upper"
            } else {
                "toupper"
            },
            &refs,
        )?,
        Op::Trim => cx.call(
            if d == Dialect::Tidyverse {
                "str_trim"
            } else {
                "trimws"
            },
            &refs,
        )?,
        Op::StartsWith | Op::EndsWith => {
            if d == Dialect::Tidyverse {
                cx.push(if op == Op::StartsWith {
                    "str_starts"
                } else {
                    "str_ends"
                });
                cx.push("(");
                cx.free(&args[0])?;
                cx.push(", fixed(");
                cx.free(&args[1])?;
                cx.push("))");
            } else {
                cx.push(if op == Op::StartsWith {
                    "startsWith("
                } else {
                    "endsWith("
                });
                cx.free(&args[0])?;
                cx.push(", ");
                cx.free(&args[1])?;
                cx.push(")");
            }
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
        Op::IsFinite => write_kind_test(d, cx, "is.finite", &args[0])?,
        Op::IsInfinite => write_kind_test(d, cx, "is.infinite", &args[0])?,
        Op::IsNan => write_kind_test(d, cx, "is.nan", &args[0])?,
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
            if super::over_numbers(&[&args[0]]) {
                cx.fidelity(NAN_DROPPED);
            }
            cx.push("sum(!is.na(");
            cx.free(&args[0])?;
            cx.push("))");
        }
        Op::RowCount => match d {
            Dialect::Tidyverse => cx.push("n()"),
            Dialect::DataTable => cx.push(".N"),
            // A bare predicate in base R has no row count to refer to.
            Dialect::Base => {
                return Err(Unsupported {
                    what: "`ROW_COUNT()`",
                    why: "a bare predicate has no row count in base R; \
                          use `nrow(t)` where the predicate is applied",
                });
            }
        },
        Op::CountDistinct => {
            if super::over_numbers(&[&args[0]]) {
                cx.fidelity(NAN_DROPPED);
            }
            match d {
                Dialect::Tidyverse => {
                    cx.push("n_distinct(");
                    cx.free(&args[0])?;
                    cx.push(", na.rm = TRUE)");
                }
                Dialect::DataTable => {
                    cx.push("uniqueN(");
                    cx.free(&args[0])?;
                    cx.push(", na.rm = TRUE)");
                }
                Dialect::Base => {
                    cx.push("length(unique(");
                    cx.free(&args[0])?;
                    cx.push("[!is.na(");
                    cx.free(&args[0])?;
                    cx.push(")]))");
                }
            }
        }
    }
    Ok(())
}

/// `is.nan` and its siblings answer `FALSE` for an `NA`, where a scalar function
/// of the language propagates a null. `is.na(x) & !is.nan(x)` is R's test for a
/// missing value that isn't a NaN, which is exactly where the two disagree.
fn write_kind_test(d: Dialect, cx: &mut Ctx, test: &str, x: &TypedExpr) -> Result<(), Unsupported> {
    cx.push(R(d).conditional());
    cx.push("(is.na(");
    cx.free(x)?;
    cx.push(") & !is.nan(");
    cx.free(x)?;
    cx.push("), NA, ");
    cx.call(test, &[x])?;
    cx.push(")");
    Ok(())
}

fn anchored(cx: &mut Ctx, pattern: &TypedExpr) -> Result<(), Unsupported> {
    // `SIMILAR TO` matches the whole string; `str_detect` and `grepl` match
    // anywhere.
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

#[cfg(test)]
mod tests {
    use super::{R, R_BASE, R_DATA_TABLE, R_TIDYVERSE};
    use crate::assert_expr::{AssertExpr, Root, check_root, lower, tests::TestEnv};
    use crate::emit::emit;

    fn translate(target: R, source: &str) -> String {
        let expr = AssertExpr::parse(source).expect("parses");
        let findings = check_root(&expr, &TestEnv, Root::Any);
        assert!(findings.is_empty(), "{source:?}: {findings:?}");
        let ir = lower(&expr, &TestEnv).expect("lowers");
        emit(&target, &ir).expect("emits").code
    }

    fn tidy(source: &str) -> String {
        translate(R_TIDYVERSE, source)
    }

    fn base(source: &str) -> String {
        translate(R_BASE, source)
    }

    fn dt(source: &str) -> String {
        translate(R_DATA_TABLE, source)
    }

    fn refused(target: R, source: &str) -> String {
        let expr = AssertExpr::parse(source).expect("parses");
        let ir = lower(&expr, &TestEnv).expect("lowers");
        let Err(unsupported) = emit(&target, &ir) else {
            panic!("{source:?} should be refused")
        };
        format!("{}: {}", unsupported.what, unsupported.why)
    }

    fn notes(target: R, source: &str) -> Vec<&'static str> {
        let expr = AssertExpr::parse(source).expect("parses");
        let ir = lower(&expr, &TestEnv).expect("lowers");
        emit(&target, &ir).expect("emits").notes
    }

    // -- Where the dialects differ, shown side by side ---------------------

    #[test]
    fn string_functions_change_spelling() {
        assert_eq!(
            tidy("LENGTH(postcode) <= 10"),
            "str_length(postcode) <= 10L"
        );
        assert_eq!(base("LENGTH(postcode) <= 10"), "nchar(postcode) <= 10L");
        assert_eq!(dt("LENGTH(postcode) <= 10"), "nchar(postcode) <= 10L");

        assert_eq!(tidy("LOWER(s) = 'a'"), "str_to_lower(s) == \"a\"");
        assert_eq!(base("LOWER(s) = 'a'"), "tolower(s) == \"a\"");

        assert_eq!(tidy("TRIM(s) = 'a'"), "str_trim(s) == \"a\"");
        assert_eq!(base("TRIM(s) = 'a'"), "trimws(s) == \"a\"");
    }

    #[test]
    fn a_literal_like_pattern_stays_literal() {
        // The tidyverse needs `fixed()`: without it the pattern is a regex,
        // so a `.` in a LIKE pattern would stop being a literal dot.
        // `startsWith` is literal already.
        assert_eq!(tidy("s LIKE 'a.c%'"), "str_starts(s, fixed(\"a.c\"))");
        assert_eq!(base("s LIKE 'a.c%'"), "startsWith(s, \"a.c\")");
        assert_eq!(dt("s LIKE 'a.c%'"), "startsWith(s, \"a.c\")");

        assert_eq!(tidy("s LIKE '%.nz'"), "str_ends(s, fixed(\".nz\"))");
        assert_eq!(base("s LIKE '%.nz'"), "endsWith(s, \".nz\")");

        assert_eq!(tidy("s LIKE 'exact'"), "s == \"exact\"");
    }

    #[test]
    fn starts_with_takes_a_computed_prefix() {
        assert_eq!(
            tidy("STARTS_WITH(s, postcode)"),
            "str_starts(s, fixed(postcode))"
        );
        assert_eq!(base("STARTS_WITH(s, postcode)"), "startsWith(s, postcode)");
    }

    #[test]
    fn similar_to_is_an_anchored_regex_match() {
        assert_eq!(
            tidy("s SIMILAR TO '[a-z]+'"),
            "str_detect(s, \"^(?:[a-z]+)$\")"
        );
        // `grepl` answers FALSE for an NA subject where the language
        // propagates the null, so its match is guarded.
        assert_eq!(
            base("s SIMILAR TO '[a-z]+'"),
            "ifelse(is.na(s), NA, grepl(\"^(?:[a-z]+)$\", s, perl = TRUE))"
        );
        assert_eq!(
            dt("s SIMILAR TO '[a-z]+'"),
            "fifelse(is.na(s), NA, grepl(\"^(?:[a-z]+)$\", s, perl = TRUE))"
        );
    }

    #[test]
    fn case_has_three_spellings() {
        let source = "CASE WHEN flag THEN qty > 1 ELSE qty > 10 END";
        assert_eq!(
            tidy(source),
            "case_when(flag ~ qty > 1L, .default = qty > 10L)"
        );
        assert_eq!(base(source), "ifelse(flag, qty > 1L, qty > 10L)");
        assert_eq!(dt(source), "fcase(flag, qty > 1L, default = qty > 10L)");

        // Without an ELSE, unmatched rows are NA, as the language says.
        let source = "CASE WHEN flag THEN qty > 1 END";
        assert_eq!(tidy(source), "case_when(flag ~ qty > 1L, .default = NA)");
        assert_eq!(base(source), "ifelse(flag, qty > 1L, NA)");
        assert_eq!(dt(source), "fcase(flag, qty > 1L, default = NA)");
    }

    #[test]
    fn between_expands_only_in_base() {
        let source = "qty BETWEEN 0 AND 100";
        assert_eq!(tidy(source), "between(qty, 0L, 100L)");
        assert_eq!(base(source), "qty >= 0L & qty <= 100L");
        assert_eq!(dt(source), "between(qty, 0L, 100L)");

        assert_eq!(
            base("qty NOT BETWEEN 0 AND 100"),
            "!(qty >= 0L & qty <= 100L)"
        );
    }

    #[test]
    fn only_the_tidyverse_keeps_a_selection() {
        // `if_all` is an ordinary value with the spec's combination and null
        // semantics; base R and data.table have no self-contained idiom, so
        // the selection expands to a conjunction.
        let source = "COLUMNS('q[34]') IS NOT NULL";
        assert_eq!(tidy(source), "if_all(matches(\"q[34]\"), \\(x) !is.na(x))");
        assert_eq!(base(source), "!is.na(q3) & !is.na(q4)");
        assert_eq!(dt(source), "!is.na(q3) & !is.na(q4)");

        assert_eq!(
            tidy("COLUMNS(*) IS NOT NULL"),
            "if_all(everything(), \\(x) !is.na(x))"
        );
    }

    #[test]
    fn row_count_is_refused_in_base() {
        assert_eq!(tidy("ROW_COUNT() > 0"), "n() > 0L");
        assert_eq!(dt("ROW_COUNT() > 0"), ".N > 0L");
        // A bare predicate in base R has no row count to refer to.
        let message = refused(R_BASE, "ROW_COUNT() > 0");
        assert!(message.contains("ROW_COUNT"), "{message}");
    }

    #[test]
    fn count_distinct_counts_the_non_null_uniques() {
        let source = "COUNT_DISTINCT(s) <= 16";
        assert_eq!(tidy(source), "n_distinct(s, na.rm = TRUE) <= 16L");
        assert_eq!(base(source), "length(unique(s[!is.na(s)])) <= 16L");
        assert_eq!(dt(source), "uniqueN(s, na.rm = TRUE) <= 16L");
    }

    #[test]
    fn a_kind_test_is_guarded_so_a_null_stays_null() {
        // `is.nan(NA)` is FALSE in R, where the language propagates the null.
        assert_eq!(
            tidy("IS_NAN(qty)"),
            "if_else(is.na(qty) & !is.nan(qty), NA, is.nan(qty))"
        );
        assert_eq!(
            base("IS_NAN(qty)"),
            "ifelse(is.na(qty) & !is.nan(qty), NA, is.nan(qty))"
        );
        assert_eq!(
            dt("IS_NAN(qty)"),
            "fifelse(is.na(qty) & !is.nan(qty), NA, is.nan(qty))"
        );

        assert_eq!(
            tidy("IS_FINITE(qty)"),
            "if_else(is.na(qty) & !is.nan(qty), NA, is.finite(qty))"
        );
        assert_eq!(
            tidy("IS_INFINITE(qty)"),
            "if_else(is.na(qty) & !is.nan(qty), NA, is.infinite(qty))"
        );
    }

    #[test]
    fn the_regex_note_names_the_dialects_flavour() {
        let source = "s SIMILAR TO '[a-z]+'";
        assert!(notes(R_TIDYVERSE, source).iter().any(|n| n.contains("ICU")));
        assert!(notes(R_BASE, source).iter().any(|n| n.contains("PCRE")));
        assert!(
            notes(R_DATA_TABLE, source)
                .iter()
                .any(|n| n.contains("PCRE"))
        );
    }

    // -- Where every dialect agrees ----------------------------------------

    #[test]
    fn columns_are_bare_names() {
        assert_eq!(tidy("qty > 0"), "qty > 0L");
        // A struct column is a data-frame column of its own.
        assert_eq!(tidy("LENGTH(addr.zip) > 0"), "str_length(addr$zip) > 0L");
        assert_eq!(
            tidy("`addr`.`nick names` IS NULL"),
            "is.na(addr$`nick names`)"
        );
    }

    #[test]
    fn an_infix_percent_operator_binds_tighter_than_arithmetic() {
        // R parses `n + 1 %in% c(1)` as `n + (1 %in% c(1))`, so the guarded
        // membership test has to bracket its own subject.
        assert_eq!(
            tidy("n + 1 IN (1, 2)"),
            "is.na(n + 1L) | ((n + 1L) %in% c(1L, 2L))"
        );
    }

    #[test]
    fn membership_is_guarded_so_a_null_still_passes() {
        // `NA %in% c(1)` is FALSE in R, where the language says null.
        for code in [
            tidy("qty IN (1, 2)"),
            base("qty IN (1, 2)"),
            dt("qty IN (1, 2)"),
        ] {
            assert_eq!(code, "is.na(qty) | (qty %in% c(1L, 2L))");
        }
        assert_eq!(tidy("qty NOT IN (1)"), "is.na(qty) | !(qty %in% c(1L))");
    }

    #[test]
    fn non_finite_literals_use_rs_own_names() {
        assert_eq!(tidy("qty = INF"), "qty == Inf");
        assert_eq!(tidy("qty = -INF"), "qty == -Inf");
        assert_eq!(tidy("qty = NAN"), "qty == NaN");
    }

    #[test]
    fn modulo_is_the_native_operator() {
        // `%%` follows the divisor, as the language does.
        assert_eq!(tidy("MOD(n, 3) = 0"), "n %% 3L == 0L");
        // `%%` binds tighter than arithmetic, so a compound operand brackets.
        assert_eq!(tidy("MOD(n + 1, 3) = 0"), "(n + 1L) %% 3L == 0L");
        assert_eq!(tidy("MOD(n, qty + 1) = 0"), "n %% (qty + 1L) == 0L");
    }

    #[test]
    fn a_shifted_date_is_promoted_first() {
        // `Date + difftime` stays a Date in R, dropping anything under a day.
        assert_eq!(
            tidy("d + interval(12, hours)"),
            "as.POSIXct(d, tz = \"UTC\") + as.difftime(12L, units = \"hours\")"
        );
        // A datetime needs no promotion.
        assert_eq!(
            dt("ts - interval(2, weeks)"),
            "ts - as.difftime(2L, units = \"weeks\")"
        );
    }

    #[test]
    fn a_computed_like_pattern_is_refused_everywhere() {
        for target in [R_TIDYVERSE, R_BASE, R_DATA_TABLE] {
            let message = refused(target, "s LIKE LOWER(postcode)");
            assert!(message.contains("computed pattern"), "{message}");
        }
    }

    #[test]
    fn aggregates_skip_nulls_and_declare_the_empty_case() {
        assert_eq!(tidy("SUM(qty) > 0"), "sum(qty, na.rm = TRUE) > 0L");
        assert_eq!(tidy("COUNT(s) > 0"), "sum(!is.na(s)) > 0L");
        assert_eq!(tidy("ANY(flag)"), "any(flag, na.rm = TRUE)");
    }

    #[test]
    fn divergences_attach_notes() {
        assert!(notes(R_TIDYVERSE, "flag").is_empty());
        assert!(notes(R_TIDYVERSE, "s = 'a'").is_empty());
        assert!(notes(R_TIDYVERSE, "qty > 0")[0].contains("NaN"));
        assert!(
            notes(R_TIDYVERSE, "ROUND(n) = n")
                .iter()
                .any(|n| n.contains("halves to even"))
        );
        assert!(
            notes(R_TIDYVERSE, "SUM(qty) > 0")
                .iter()
                .any(|n| n.contains("identity"))
        );
        assert!(
            notes(R_TIDYVERSE, "SUM(qty) > 0")
                .iter()
                .any(|n| n.contains("na.rm = TRUE"))
        );
        assert!(
            notes(R_TIDYVERSE, "qty IS NULL")
                .iter()
                .any(|n| n.contains("is.na` is TRUE"))
        );
    }
}
