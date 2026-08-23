//! [`RExpr`] to the language's own [`Expr`], with the divergences noted.
//!
//! Runs after [`fold`](super::fold) has rewritten the idioms that mean exactly
//! one of the language's constructs, so what arrives here is mapped a node at a
//! time. Where a construct means something slightly different in R than the
//! reading it is given, a note says so — never a guard, which would put a rule
//! in the dictionary that its author didn't write.

use super::ast::{RArg, RBinop, RExpr, RKind, RUnop};
use super::fold::{self, Folded};
use crate::assert_expr::{
    ArithOp, CmpOp, ColumnsSelector, Expr, ExprKind, Named, NumLit, ParseError,
};
use crate::emit::r_notes as notes;
use crate::parse::{Notes, untranslatable};

/// The unguarded `%in%`, which the R emitter never produces — it always writes
/// the null guard — but an author writes all the time.
const BARE_IN: &str = "R's `%in%` answers FALSE for an NA subject, where data-dict's `IN` gives null and so passes; a row whose value is missing is reported there and not here.";

/// An aggregate written without `na.rm = TRUE`, which the language has no way
/// to spell: its aggregates always skip nulls.
const NA_NOT_REMOVED: &str = "an aggregate without `na.rm = TRUE` yields NA in R as soon as one value is missing, where data-dict folds the values that are present.";

pub struct Mapper<'a> {
    pub notes: &'a mut Notes,
    /// Inside an `if_all`, the lambda's parameter and the selection it stands
    /// for. A mention of that name reads as the selection itself, which is how
    /// the language writes the same thing.
    pub selected: Option<(String, ExprKind)>,
}

impl<'a> Mapper<'a> {
    pub fn new(notes: &'a mut Notes) -> Mapper<'a> {
        Mapper {
            notes,
            selected: None,
        }
    }
}

impl Mapper<'_> {
    pub fn expr(&mut self, e: &RExpr) -> Result<Expr, ParseError> {
        // An idiom that stands for exactly one construct is read as that
        // construct, before the node it is built out of gets its own reading.
        if let Some(folded) = fold::recognise(e) {
            return self.folded(folded, e.span);
        }
        let span = e.span;
        let kind = match &e.kind {
            RKind::Num { value, integer } => ExprKind::Number(number(*value, *integer)),
            RKind::Str(s) => ExprKind::Str(s.clone()),
            RKind::Logical(b) => ExprKind::Bool(*b),
            RKind::Na => ExprKind::Null,
            RKind::Inf => ExprKind::Number(NumLit::Float(f64::INFINITY)),
            RKind::NaN => ExprKind::Number(NumLit::Float(f64::NAN)),
            // R's `NULL` is an absent value, not a missing one; the language's
            // `NULL` is R's `NA`, which is a different thing entirely.
            RKind::Null => {
                return Err(untranslatable(
                    "`NULL`",
                    "R's `NULL` is an absent value, not a missing one; `NA` is the missing value \
                     the language calls `NULL`",
                    span.0,
                ));
            }
            RKind::Name(name) => return self.name(name, span),
            RKind::Dollar(..) => ExprKind::Column(self.path(e)?),
            RKind::Index(..) => {
                return Err(untranslatable(
                    "`[`",
                    "the language has no subsetting; a rule is written over whole columns",
                    span.0,
                ));
            }
            RKind::Lambda { .. } => {
                return Err(untranslatable(
                    "a function",
                    "the language has no lambdas outside a column selection",
                    span.0,
                ));
            }
            RKind::Unary(RUnop::Neg, x) => ExprKind::Neg(Box::new(self.expr(x)?)),
            // `!is.na(x)` is one construct in the language, not a negated one —
            // and it is how `IS NOT NULL` is emitted, so it has to read back
            // that way.
            RKind::Unary(RUnop::Not, x) => match x.as_is_na() {
                Some(operand) => {
                    self.notes.add(notes::NAN_IS_MISSING);
                    ExprKind::IsNull {
                        operand: Box::new(self.expr(operand)?),
                        negated: true,
                    }
                }
                None => ExprKind::Not(Box::new(self.expr(x)?)),
            },
            RKind::Binary(op, lhs, rhs) => return self.binary(*op, lhs, rhs, span),
            RKind::Call { fun, args } => return self.call(fun, args, span),
        };
        Ok(Expr {
            kind,
            start: span.0,
            end: span.1,
        })
    }

    /// Read a recognised idiom as the construct it stands for. Each of these is
    /// exact about nulls — that is what the guard was for — so none carries a
    /// note of its own beyond the ones its operands earn.
    fn folded(&mut self, folded: Folded<'_>, span: (usize, usize)) -> Result<Expr, ParseError> {
        let kind = match folded {
            Folded::Unguarded(body) => return self.expr(body),
            Folded::ExactIn {
                needle,
                list,
                negated,
            } => {
                self.notes.add(notes::NAN_COMPARISON);
                ExprKind::In {
                    operand: Box::new(self.expr(needle)?),
                    list: self.list(list)?,
                    negated,
                }
            }
            Folded::Count(operand) => {
                self.notes.add(notes::NAN_DROPPED);
                self.func("COUNT", &[operand])?
            }
            Folded::CountDistinct(operand) => {
                self.notes.add(notes::EMPTY_FOLD);
                self.notes.add(notes::NAN_DROPPED);
                self.func("COUNT_DISTINCT", &[operand])?
            }
            Folded::Between { operand, lo, hi } => {
                self.notes.add(notes::NAN_COMPARISON);
                ExprKind::Between {
                    operand: Box::new(self.expr(operand)?),
                    lo: Box::new(self.expr(lo)?),
                    hi: Box::new(self.expr(hi)?),
                    negated: false,
                }
            }
        };
        Ok(Expr {
            kind,
            start: span.0,
            end: span.1,
        })
    }

    fn name(&mut self, name: &str, span: (usize, usize)) -> Result<Expr, ParseError> {
        // Inside an `if_all`, the lambda parameter is the selected column.
        if let Some((param, selection)) = &self.selected
            && param == name
        {
            return Ok(Expr {
                kind: selection.clone(),
                start: span.0,
                end: span.1,
            });
        }
        // data.table's row count is a name, not a call.
        let kind = if name == ".N" {
            ExprKind::Call {
                name: "ROW_COUNT".to_string(),
                args: Vec::new(),
            }
        } else {
            ExprKind::Column(vec![name.to_string()])
        };
        Ok(Expr {
            kind,
            start: span.0,
            end: span.1,
        })
    }

    /// A `$` chain, which is how R reaches a struct column's field.
    fn path(&mut self, e: &RExpr) -> Result<Vec<String>, ParseError> {
        match &e.kind {
            RKind::Name(name) => Ok(vec![name.clone()]),
            RKind::Dollar(base, field) => {
                let mut path = self.path(base)?;
                path.push(field.clone());
                Ok(path)
            }
            _ => Err(untranslatable(
                "`$`",
                "only a column's field can be reached with `$`",
                e.span.0,
            )),
        }
    }

    fn binary(
        &mut self,
        op: RBinop,
        lhs: &RExpr,
        rhs: &RExpr,
        span: (usize, usize),
    ) -> Result<Expr, ParseError> {
        let kind = match op {
            RBinop::Add | RBinop::Sub | RBinop::Mul | RBinop::Div => {
                let arith = match op {
                    RBinop::Add => ArithOp::Add,
                    RBinop::Sub => ArithOp::Sub,
                    RBinop::Mul => ArithOp::Mul,
                    _ => ArithOp::Div,
                };
                ExprKind::Arith {
                    op: arith,
                    lhs: Box::new(self.expr(lhs)?),
                    rhs: Box::new(self.expr(rhs)?),
                }
            }
            RBinop::Eq | RBinop::Ne | RBinop::Lt | RBinop::Le | RBinop::Gt | RBinop::Ge => {
                let cmp = match op {
                    RBinop::Eq => CmpOp::Eq,
                    RBinop::Ne => CmpOp::Ne,
                    RBinop::Lt => CmpOp::Lt,
                    RBinop::Le => CmpOp::Le,
                    RBinop::Gt => CmpOp::Gt,
                    _ => CmpOp::Ge,
                };
                self.notes.add(notes::NAN_COMPARISON);
                ExprKind::Compare {
                    op: cmp,
                    lhs: Box::new(self.expr(lhs)?),
                    rhs: Box::new(self.expr(rhs)?),
                }
            }
            RBinop::And => ExprKind::And(Box::new(self.expr(lhs)?), Box::new(self.expr(rhs)?)),
            RBinop::Or => ExprKind::Or(Box::new(self.expr(lhs)?), Box::new(self.expr(rhs)?)),
            RBinop::Mod => {
                self.notes.add(notes::MODULO_ZERO);
                ExprKind::Call {
                    name: "MOD".to_string(),
                    args: vec![self.expr(lhs)?, self.expr(rhs)?],
                }
            }
            // An unguarded `%in%`: the reading is `IN`, which differs on a null
            // subject. The guarded form never reaches here — `fold` has already
            // recognised it as an exact `IN`.
            RBinop::In => {
                self.notes.add(BARE_IN);
                ExprKind::In {
                    operand: Box::new(self.expr(lhs)?),
                    list: self.list(rhs)?,
                    negated: false,
                }
            }
            RBinop::Formula => {
                return Err(untranslatable(
                    "`~`",
                    "a formula has meaning only inside `case_when`",
                    span.0,
                ));
            }
        };
        Ok(Expr {
            kind,
            start: span.0,
            end: span.1,
        })
    }

    /// The `c(...)` on the right of `%in%`.
    pub fn list(&mut self, e: &RExpr) -> Result<Vec<Expr>, ParseError> {
        let Some(args) = e.as_call("c") else {
            return Err(untranslatable(
                "`%in%` over a computed set",
                "the language's `IN` takes a written-out list, so the right side must be `c(...)`",
                e.span.0,
            ));
        };
        args.iter().map(|arg| self.expr(&arg.value)).collect()
    }

    fn call(&mut self, fun: &str, args: &[RArg], span: (usize, usize)) -> Result<Expr, ParseError> {
        // Every function below takes its arguments positionally; `na.rm` is the
        // one named argument, and the aggregates strip it themselves.
        if let Some(kind) = self.aggregate(fun, args, span)? {
            return Ok(Expr {
                kind,
                start: span.0,
                end: span.1,
            });
        }
        let kind = match fun {
            // -- strings, one name per language construct --
            "nchar" | "str_length" => self.func("LENGTH", &positional(args)?)?,
            "tolower" | "str_to_lower" => self.func("LOWER", &positional(args)?)?,
            "toupper" | "str_to_upper" => self.func("UPPER", &positional(args)?)?,
            "trimws" | "str_trim" => self.func("TRIM", &positional(args)?)?,
            "startsWith" | "str_starts" => self.affix("STARTS_WITH", &positional(args)?, span)?,
            "endsWith" | "str_ends" => self.affix("ENDS_WITH", &positional(args)?, span)?,

            // -- numbers --
            "abs" => self.func("ABS", &positional(args)?)?,
            "floor" => self.func("FLOOR", &positional(args)?)?,
            "ceiling" => self.func("CEIL", &positional(args)?)?,
            "round" => {
                self.notes.add(notes::ROUNDING);
                self.func("ROUND", &positional(args)?)?
            }
            "is.nan" => self.func("IS_NAN", &positional(args)?)?,
            "is.finite" => self.func("IS_FINITE", &positional(args)?)?,
            "is.infinite" => self.func("IS_INFINITE", &positional(args)?)?,

            // -- null tests --
            "is.na" => {
                let args = positional(args)?;
                let [operand] = args[..] else {
                    return Err(arity(fun, 1, args.len(), span));
                };
                self.notes.add(notes::NAN_IS_MISSING);
                ExprKind::IsNull {
                    operand: Box::new(self.expr(operand)?),
                    negated: false,
                }
            }

            // -- time --
            "Sys.time" => ExprKind::Now,
            // The language has no temporal literal: a date is a string that
            // becomes one beside a temporal column, which is what these say.
            "as.Date" => return self.temporal(fun, &positional(args)?, span),
            "as.POSIXct" => return self.temporal_with_zone(fun, args, span),
            "as.difftime" => return self.interval(args, span),

            // -- pattern matching --
            "grepl" | "str_detect" => return self.detect(fun, args, span),

            // -- conditionals --
            "ifelse" | "if_else" | "fifelse" => return self.if_else(fun, &positional(args)?, span),
            "case_when" => return self.case_when(args, span),
            "fcase" => return self.fcase(args, span),

            // dplyr's and data.table's `between` are both `x >= lo & x <= hi`
            // underneath, which is what the language's `BETWEEN` is.
            "between" => {
                let args = positional(args)?;
                let [operand, lo, hi] = args[..] else {
                    return Err(arity(fun, 3, args.len(), span));
                };
                self.notes.add(notes::NAN_COMPARISON);
                ExprKind::Between {
                    operand: Box::new(self.expr(operand)?),
                    lo: Box::new(self.expr(lo)?),
                    hi: Box::new(self.expr(hi)?),
                    negated: false,
                }
            }

            "if_all" => return self.if_all(&positional(args)?, span),
            "if_any" => {
                return Err(untranslatable(
                    "`if_any`",
                    "the language's `COLUMNS(...)` combines a selection with AND only",
                    span.0,
                ));
            }
            other => {
                return Err(untranslatable(
                    format!("`{other}()`"),
                    "the language has no equivalent",
                    span.0,
                ));
            }
        };
        Ok(Expr {
            kind,
            start: span.0,
            end: span.1,
        })
    }

    /// A plain call, mapped name for name.
    fn func(&mut self, name: &str, args: &[&RExpr]) -> Result<ExprKind, ParseError> {
        Ok(ExprKind::Call {
            name: name.to_string(),
            args: args
                .iter()
                .map(|a| self.expr(a))
                .collect::<Result<_, _>>()?,
        })
    }

    /// `startsWith`/`str_starts` and their `ends` siblings. stringr's takes the
    /// pattern in `fixed(...)`, which says the pattern is literal — exactly what
    /// the language's own function means, so the wrapper is dropped.
    fn affix(
        &mut self,
        name: &str,
        args: &[&RExpr],
        span: (usize, usize),
    ) -> Result<ExprKind, ParseError> {
        let [subject, pattern] = args[..] else {
            return Err(arity(name, 2, args.len(), span));
        };
        let pattern = match pattern.as_plain_call("fixed", 1) {
            Some(inner) => inner[0],
            None => pattern,
        };
        self.func(name, &[subject, pattern])
    }

    fn temporal(
        &mut self,
        fun: &str,
        args: &[&RExpr],
        span: (usize, usize),
    ) -> Result<Expr, ParseError> {
        let [inner] = args[..] else {
            return Err(arity(fun, 1, args.len(), span));
        };
        let Some(text) = inner.as_str() else {
            return Err(untranslatable(
                format!("`{fun}()` of a computed value"),
                "the language writes a date as a literal string, so only a literal converts",
                span.0,
            ));
        };
        Ok(Expr {
            kind: ExprKind::Str(text.to_string()),
            start: span.0,
            end: span.1,
        })
    }

    /// `as.POSIXct(x, tz = "UTC")`. Any other zone would change the instant the
    /// string denotes, and the language reads a zoneless literal as the column's
    /// own zone rather than the machine's.
    fn temporal_with_zone(
        &mut self,
        fun: &str,
        args: &[RArg],
        span: (usize, usize),
    ) -> Result<Expr, ParseError> {
        let mut positional = Vec::new();
        for arg in args {
            match arg.name.as_deref() {
                None => positional.push(&arg.value),
                Some("tz") => match arg.value.as_str() {
                    Some("UTC") => {}
                    _ => {
                        return Err(untranslatable(
                            "`as.POSIXct(tz = …)` in a zone other than UTC",
                            "a zoneless literal is read in the column's own zone, so any other \
                             zone would change the instant it names",
                            arg.value.span.0,
                        ));
                    }
                },
                Some(other) => {
                    return Err(untranslatable(
                        format!("`{other} =`"),
                        "only `tz` is understood here",
                        arg.value.span.0,
                    ));
                }
            }
        }
        // Around anything but a literal, this is the promotion an emitter adds
        // before shifting a date: R's `Date + difftime` stays a `Date` and
        // silently drops anything shorter than a day, where the language gives a
        // datetime. The language promotes on its own, so the wrapper is dropped.
        if let [inner] = positional[..]
            && inner.as_str().is_none()
        {
            return self.expr(inner);
        }
        self.temporal(fun, &positional, span)
    }

    /// `as.difftime(n, units = "weeks")`.
    fn interval(&mut self, args: &[RArg], span: (usize, usize)) -> Result<Expr, ParseError> {
        let mut count = None;
        let mut units = None;
        for arg in args {
            match arg.name.as_deref() {
                None if count.is_none() => count = Some(&arg.value),
                Some("units") => units = arg.value.as_str(),
                _ => {
                    return Err(untranslatable(
                        "`as.difftime()` in this form",
                        "the language spells a duration `interval(<n>, <unit>)`",
                        arg.value.span.0,
                    ));
                }
            }
        }
        let (Some(count), Some(units)) = (count, units) else {
            return Err(untranslatable(
                "`as.difftime()` without `units`",
                "the language's `interval` always names its unit",
                span.0,
            ));
        };
        // R abbreviates; the language writes the unit out.
        let unit = match units {
            "secs" => "seconds",
            "mins" => "minutes",
            "hours" => "hours",
            "days" => "days",
            "weeks" => "weeks",
            other => {
                return Err(untranslatable(
                    format!("`units = \"{other}\"`"),
                    "the language's fixed-length units are seconds, minutes, hours, days and weeks",
                    span.0,
                ));
            }
        };
        let n = self.expr(count)?;
        Ok(Expr {
            kind: ExprKind::Interval {
                n: Box::new(n),
                unit: unit.to_string(),
                unit_start: span.0,
                unit_end: span.1,
            },
            start: span.0,
            end: span.1,
        })
    }

    /// `grepl(pattern, x)` and `str_detect(x, pattern)` — note the argument
    /// order differs. Both match anywhere in the string; `SIMILAR TO` matches
    /// the whole of it, so an unanchored pattern gains the wildcards that say so.
    fn detect(
        &mut self,
        fun: &str,
        args: &[RArg],
        span: (usize, usize),
    ) -> Result<Expr, ParseError> {
        let mut plain = Vec::new();
        for arg in args {
            match arg.name.as_deref() {
                None => plain.push(&arg.value),
                // Which regex engine `grepl` uses. Either way it isn't RE2, and
                // the note below says so, so the choice changes nothing here.
                Some("perl") | Some("fixed") | Some("ignore.case") if fun == "grepl" => {}
                Some(other) => {
                    return Err(untranslatable(
                        format!("`{other} =`"),
                        "the language matches with one fixed regular-expression syntax",
                        arg.value.span.0,
                    ));
                }
            }
        }
        let args = plain;
        let (subject, pattern) = match (fun, &args[..]) {
            ("grepl", [pattern, subject]) => (*subject, *pattern),
            ("str_detect", [subject, pattern]) => (*subject, *pattern),
            _ => return Err(arity(fun, 2, args.len(), span)),
        };
        self.notes.add(if fun == "grepl" {
            notes::REGEX_PCRE
        } else {
            notes::REGEX_ICU
        });
        let Some(text) = pattern.as_str() else {
            return Err(untranslatable(
                format!("`{fun}()` with a computed pattern"),
                "the pattern has to be a literal so it can be anchored",
                pattern.span.0,
            ));
        };
        let anchored = unanchor(text);
        Ok(Expr {
            kind: ExprKind::SimilarTo {
                operand: Box::new(self.expr(subject)?),
                pattern: Box::new(Expr {
                    kind: ExprKind::Str(anchored),
                    start: pattern.span.0,
                    end: pattern.span.1,
                }),
                negated: false,
            },
            start: span.0,
            end: span.1,
        })
    }

    /// `if_all(selection, \(x) body)` — the tidyverse's own spelling of the
    /// language's `COLUMNS(...)`, and the only R form that keeps a selection as
    /// a selection. The lambda's parameter stands for the selected column, so
    /// every mention of it inside `body` becomes the selection itself.
    fn if_all(&mut self, args: &[&RExpr], span: (usize, usize)) -> Result<Expr, ParseError> {
        let [selector, lambda] = args[..] else {
            return Err(arity("if_all", 2, args.len(), span));
        };
        let RKind::Lambda { param, body } = &lambda.kind else {
            return Err(untranslatable(
                "`if_all()` without a function",
                "the predicate is written `\\(x) …`, with `x` standing for each column",
                lambda.span.0,
            ));
        };
        let selection = self.selector(selector)?;
        let outer = self.selected.replace((param.clone(), selection));
        let result = self.expr(body);
        self.selected = outer;
        result
    }

    /// What `if_all` is applied to: `everything()`, `matches("re")`, or a
    /// `c(...)` of column names.
    fn selector(&mut self, e: &RExpr) -> Result<ExprKind, ParseError> {
        if e.as_plain_call("everything", 0).is_some() {
            return Ok(ExprKind::Columns(ColumnsSelector::All));
        }
        if let Some(args) = e.as_plain_call("matches", 1) {
            let Some(pattern) = args[0].as_str() else {
                return Err(untranslatable(
                    "`matches()` with a computed pattern",
                    "a selection's pattern has to be a literal",
                    args[0].span.0,
                ));
            };
            return Ok(ExprKind::Columns(ColumnsSelector::Regex {
                pattern: pattern.to_string(),
                start: args[0].span.0,
                end: args[0].span.1,
            }));
        }
        if let Some(args) = e.as_call("c") {
            let mut named = Vec::new();
            for arg in args {
                let RKind::Name(name) = &arg.value.kind else {
                    return Err(untranslatable(
                        "a column selection that is not a name",
                        "the list form of a selection names its columns",
                        arg.value.span.0,
                    ));
                };
                named.push(Named {
                    name: name.clone(),
                    start: arg.value.span.0,
                    end: arg.value.span.1,
                });
            }
            return Ok(ExprKind::Columns(ColumnsSelector::List(named)));
        }
        Err(untranslatable(
            "this column selection",
            "the language selects with `COLUMNS(*)`, a pattern, or a list of names",
            e.span.0,
        ))
    }

    /// The two-branch conditional. Two shapes collapse: a trailing `NA` branch
    /// is dropped, since a `CASE` with no `ELSE` already gives null, and a
    /// conditional in the `else` position is absorbed as a further `WHEN`, which
    /// is how base R spells a multi-branch `CASE`.
    fn if_else(
        &mut self,
        fun: &str,
        args: &[&RExpr],
        span: (usize, usize),
    ) -> Result<Expr, ParseError> {
        let [cond, then, otherwise] = args[..] else {
            return Err(arity(fun, 3, args.len(), span));
        };
        let mut whens = vec![(self.expr(cond)?, self.expr(then)?)];
        let mut tail = otherwise;
        let els = loop {
            if tail.is_na() {
                break None;
            }
            let nested = ["ifelse", "if_else", "fifelse"]
                .iter()
                .find_map(|name| tail.as_plain_call(name, 3));
            match nested {
                Some(inner) => {
                    whens.push((self.expr(inner[0])?, self.expr(inner[1])?));
                    tail = inner[2];
                }
                None => break Some(Box::new(self.expr(tail)?)),
            }
        };
        Ok(Expr {
            kind: ExprKind::Case { whens, els },
            start: span.0,
            end: span.1,
        })
    }

    /// dplyr's `case_when(cond ~ value, …, .default = other)`.
    fn case_when(&mut self, args: &[RArg], span: (usize, usize)) -> Result<Expr, ParseError> {
        let mut whens = Vec::new();
        let mut els = None;
        for arg in args {
            match arg.name.as_deref() {
                Some(".default") | Some(".missing") => {
                    if !arg.value.is_na() {
                        els = Some(Box::new(self.expr(&arg.value)?));
                    }
                }
                Some(other) => {
                    return Err(untranslatable(
                        format!("`{other} =`"),
                        "`case_when` takes formulas and `.default`",
                        arg.value.span.0,
                    ));
                }
                None => {
                    let Some((cond, result)) = arg.value.as_binary(RBinop::Formula) else {
                        return Err(untranslatable(
                            "a `case_when` branch that is not a formula",
                            "each branch is written `condition ~ value`",
                            arg.value.span.0,
                        ));
                    };
                    // The pre-`.default` idiom: a catch-all `TRUE ~ x`.
                    if matches!(cond.kind, RKind::Logical(true)) {
                        els = Some(Box::new(self.expr(result)?));
                    } else {
                        whens.push((self.expr(cond)?, self.expr(result)?));
                    }
                }
            }
        }
        self.finish_case(whens, els, span)
    }

    /// data.table's `fcase(cond, value, …, default = other)`, which pairs its
    /// arguments positionally instead of with a formula.
    fn fcase(&mut self, args: &[RArg], span: (usize, usize)) -> Result<Expr, ParseError> {
        let mut whens = Vec::new();
        let mut els = None;
        let mut pending: Option<&RExpr> = None;
        for arg in args {
            match arg.name.as_deref() {
                Some("default") => {
                    if !arg.value.is_na() {
                        els = Some(Box::new(self.expr(&arg.value)?));
                    }
                }
                Some(other) => {
                    return Err(untranslatable(
                        format!("`{other} =`"),
                        "`fcase` takes condition and value in turn, and `default`",
                        arg.value.span.0,
                    ));
                }
                None => match pending.take() {
                    None => pending = Some(&arg.value),
                    Some(cond) => whens.push((self.expr(cond)?, self.expr(&arg.value)?)),
                },
            }
        }
        if let Some(dangling) = pending {
            return Err(untranslatable(
                "an `fcase` condition with no value",
                "each condition is followed by the value it gives",
                dangling.span.0,
            ));
        }
        self.finish_case(whens, els, span)
    }

    fn finish_case(
        &mut self,
        whens: Vec<(Expr, Expr)>,
        els: Option<Box<Expr>>,
        span: (usize, usize),
    ) -> Result<Expr, ParseError> {
        if whens.is_empty() {
            return Err(untranslatable(
                "a conditional with no branches",
                "the language's `CASE` needs at least one `WHEN`",
                span.0,
            ));
        }
        Ok(Expr {
            kind: ExprKind::Case { whens, els },
            start: span.0,
            end: span.1,
        })
    }

    /// The folds, which all take `na.rm = TRUE` and drop it. Returns `None`
    /// when `fun` isn't one of them, so the caller carries on.
    fn aggregate(
        &mut self,
        fun: &str,
        args: &[RArg],
        span: (usize, usize),
    ) -> Result<Option<ExprKind>, ParseError> {
        let name = match fun {
            "sum" => "SUM",
            "mean" => "AVG",
            "min" => "MIN",
            "max" => "MAX",
            "any" => "ANY",
            "all" => "ALL",
            "n_distinct" | "uniqueN" => "COUNT_DISTINCT",
            "n" => "ROW_COUNT",
            _ => return Ok(None),
        };
        if name == "ROW_COUNT" {
            return Ok(Some(ExprKind::Call {
                name: name.to_string(),
                args: Vec::new(),
            }));
        }
        let mut operand = None;
        let mut na_rm = false;
        for arg in args {
            match arg.name.as_deref() {
                None if operand.is_none() => operand = Some(&arg.value),
                // `na.rm = FALSE` is not refused: it is what leaving `na.rm`
                // out already means, and the note below covers both.
                Some("na.rm") => match arg.value.kind {
                    RKind::Logical(value) => na_rm = value,
                    _ => {
                        return Err(untranslatable(
                            "`na.rm` from a computed value",
                            "the language's aggregates always skip nulls, so this has to be \
                             decided when the rule is written",
                            arg.value.span.0,
                        ));
                    }
                },
                _ => {
                    return Err(untranslatable(
                        format!("`{fun}()` in this form"),
                        "the language's aggregates take one column",
                        arg.value.span.0,
                    ));
                }
            }
        }
        let Some(operand) = operand else {
            return Err(arity(fun, 1, 0, span));
        };
        self.notes.add(notes::EMPTY_FOLD);
        if !na_rm {
            self.notes.add(NA_NOT_REMOVED);
        }
        if name == "SUM" {
            self.notes.add(notes::OVERFLOW);
        }
        if matches!(name, "SUM" | "AVG" | "MIN" | "MAX" | "COUNT_DISTINCT") {
            self.notes.add(notes::NAN_DROPPED);
        }
        Ok(Some(ExprKind::Call {
            name: name.to_string(),
            args: vec![self.expr(operand)?],
        }))
    }
}

/// A numeric literal. R writes every number as a double, so what decides the
/// language's reading is whether the source said integer — an `L` suffix, or no
/// point and no exponent.
fn number(value: f64, integer: bool) -> NumLit {
    if integer && value.fract() == 0.0 && value.abs() <= i64::MAX as f64 {
        NumLit::Int(value as i64)
    } else {
        NumLit::Float(value)
    }
}

fn arity(fun: &str, want: usize, got: usize, span: (usize, usize)) -> ParseError {
    ParseError {
        message: format!("`{fun}()` takes {want} arguments, but {got} were given"),
        at: span.0,
    }
}

/// A call's arguments when none is named, which is every function the language
/// has: it takes its arguments in order.
fn positional(args: &[RArg]) -> Result<Vec<&RExpr>, ParseError> {
    if let Some(named) = args.iter().find(|a| a.name.is_some()) {
        let name = named.name.clone().unwrap_or_default();
        return Err(untranslatable(
            format!("`{name} =`"),
            "the language's functions take their arguments in order",
            named.value.span.0,
        ));
    }
    Ok(args.iter().map(|a| &a.value).collect())
}

/// Turn a pattern that matches *anywhere* into one that matches the whole
/// string, which is what `SIMILAR TO` does.
///
/// Two anchored forms come back from the emitters and have their anchors
/// removed rather than doubled: `^(?:…)$`, which is how a `SIMILAR TO` is
/// written for R, and `^…$`, which is how a `LIKE` pattern's regex is built.
/// Anything else grows the wildcards that say "anywhere" — inside a group,
/// because a bare `.*a|b.*` would regroup around the alternation.
fn unanchor(pattern: &str) -> String {
    if let Some(inner) = pattern
        .strip_prefix("^(?:")
        .and_then(|rest| rest.strip_suffix(")$"))
    {
        return inner.to_string();
    }
    if let Some(inner) = pattern
        .strip_prefix('^')
        .and_then(|rest| rest.strip_suffix('$'))
        // `^a|b$` is `(^a)|(b$)`, so its anchors are not the whole pattern's
        // and removing them would change what it matches.
        && !has_top_level_alternation(inner)
    {
        return inner.to_string();
    }
    format!(".*(?:{pattern}).*")
}

/// Whether `pattern` has a `|` outside every group and character class.
fn has_top_level_alternation(pattern: &str) -> bool {
    let mut depth = 0usize;
    let mut in_class = false;
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                chars.next();
            }
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' if !in_class => depth += 1,
            ')' if !in_class => depth = depth.saturating_sub(1),
            '|' if !in_class && depth == 0 => return true,
            _ => {}
        }
    }
    false
}
