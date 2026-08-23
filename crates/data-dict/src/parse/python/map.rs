//! [`PyExpr`] to the language's own [`Expr`], with the divergences noted.
//!
//! Much shorter than [the R mapper](super::super::r::map) would suggest,
//! because polars needs almost no guards: `is_null`, `is_in`, `is_nan` and the
//! string methods all propagate a null the way the language does, so what an
//! emitter wrote is what a reader reads. Only two shapes are folded back — the
//! `drop_nulls()` before `n_unique()`, and the cast that promotes a date before
//! a duration is added to it.

use super::ast::{PyArg, PyBinop, PyExpr, PyKind, PyUnop};
use crate::assert_expr::{
    ArithOp, CmpOp, ColumnsSelector, Expr, ExprKind, Named, NumLit, ParseError,
};
use crate::emit::polars_notes as notes;
use crate::parse::{Notes, untranslatable};

/// polars needs no scope of its own while reading: a selection is spelled with
/// the same `pl.col` a single column is, so the body of an `all_horizontal`
/// reads exactly as it stands.
pub struct Mapper<'a> {
    pub notes: &'a mut Notes,
}

impl<'a> Mapper<'a> {
    pub fn new(notes: &'a mut Notes) -> Mapper<'a> {
        Mapper { notes }
    }
}

impl Mapper<'_> {
    pub fn expr(&mut self, e: &PyExpr) -> Result<Expr, ParseError> {
        let span = e.span;
        let kind = match &e.kind {
            PyKind::Int(n) => ExprKind::Number(NumLit::Int(*n)),
            PyKind::Float(x) => ExprKind::Number(NumLit::Float(*x)),
            PyKind::Str(s) => ExprKind::Str(s.clone()),
            PyKind::Bool(b) => ExprKind::Bool(*b),
            PyKind::None => ExprKind::Null,
            PyKind::List(_) => {
                return Err(untranslatable(
                    "a list",
                    "a list has meaning only as the right-hand side of `is_in`",
                    span.0,
                ));
            }
            PyKind::Name(name) => {
                return Err(untranslatable(
                    format!("`{name}`"),
                    "a column is written `pl.col(\"name\")`, not as a bare name",
                    span.0,
                ));
            }
            PyKind::Attr(..) => {
                return Err(untranslatable(
                    format!("`{}`", e.dotted().unwrap_or_else(|| "this".into())),
                    "the language has no equivalent",
                    span.0,
                ));
            }
            PyKind::Unary(PyUnop::Neg, x) => ExprKind::Neg(Box::new(self.expr(x)?)),
            PyKind::Unary(PyUnop::Invert, x) => return self.invert(x, span),
            PyKind::Binary(op, lhs, rhs) => return self.binary(*op, lhs, rhs, span),
            PyKind::Call { .. } => return self.call(e, span),
        };
        Ok(Expr {
            kind,
            start: span.0,
            end: span.1,
        })
    }

    /// `~x`. Several of the emitted forms put it in front of a construct the
    /// language spells with its own negation, so those fold rather than
    /// becoming a `NOT` around them.
    fn invert(&mut self, x: &PyExpr, span: (usize, usize)) -> Result<Expr, ParseError> {
        let inner = self.expr(x)?;
        let kind = match inner.kind {
            ExprKind::IsNull { operand, negated } => ExprKind::IsNull {
                operand,
                negated: !negated,
            },
            ExprKind::In {
                operand,
                list,
                negated,
            } => ExprKind::In {
                operand,
                list,
                negated: !negated,
            },
            ExprKind::Between {
                operand,
                lo,
                hi,
                negated,
            } => ExprKind::Between {
                operand,
                lo,
                hi,
                negated: !negated,
            },
            ExprKind::Like {
                operand,
                pattern,
                negated,
            } => ExprKind::Like {
                operand,
                pattern,
                negated: !negated,
            },
            ExprKind::SimilarTo {
                operand,
                pattern,
                negated,
            } => ExprKind::SimilarTo {
                operand,
                pattern,
                negated: !negated,
            },
            other => ExprKind::Not(Box::new(Expr {
                kind: other,
                start: inner.start,
                end: inner.end,
            })),
        };
        Ok(Expr {
            kind,
            start: span.0,
            end: span.1,
        })
    }

    fn binary(
        &mut self,
        op: PyBinop,
        lhs: &PyExpr,
        rhs: &PyExpr,
        span: (usize, usize),
    ) -> Result<Expr, ParseError> {
        // A date promoted before a duration is added to it: the emitter's own
        // shape, which the language does on its own.
        let kind = match op {
            PyBinop::Add | PyBinop::Sub | PyBinop::Mul | PyBinop::Div => {
                let arith = match op {
                    PyBinop::Add => ArithOp::Add,
                    PyBinop::Sub => ArithOp::Sub,
                    PyBinop::Mul => ArithOp::Mul,
                    _ => ArithOp::Div,
                };
                if matches!(op, PyBinop::Add | PyBinop::Sub | PyBinop::Mul) {
                    self.notes.add(notes::OVERFLOW);
                }
                ExprKind::Arith {
                    op: arith,
                    lhs: Box::new(self.expr(strip_promotion(lhs))?),
                    rhs: Box::new(self.expr(rhs)?),
                }
            }
            PyBinop::Mod => ExprKind::Call {
                name: "MOD".to_string(),
                args: vec![self.expr(lhs)?, self.expr(rhs)?],
            },
            PyBinop::Eq | PyBinop::Ne | PyBinop::Lt | PyBinop::Le | PyBinop::Gt | PyBinop::Ge => {
                let cmp = match op {
                    PyBinop::Eq => CmpOp::Eq,
                    PyBinop::Ne => CmpOp::Ne,
                    PyBinop::Lt => CmpOp::Lt,
                    PyBinop::Le => CmpOp::Le,
                    PyBinop::Gt => CmpOp::Gt,
                    _ => CmpOp::Ge,
                };
                self.notes.add(notes::NAN_COMPARISON);
                ExprKind::Compare {
                    op: cmp,
                    lhs: Box::new(self.expr(lhs)?),
                    rhs: Box::new(self.expr(rhs)?),
                }
            }
            PyBinop::BitAnd => ExprKind::And(Box::new(self.expr(lhs)?), Box::new(self.expr(rhs)?)),
            PyBinop::BitOr => ExprKind::Or(Box::new(self.expr(lhs)?), Box::new(self.expr(rhs)?)),
        };
        Ok(Expr {
            kind,
            start: span.0,
            end: span.1,
        })
    }

    fn call(&mut self, e: &PyExpr, span: (usize, usize)) -> Result<Expr, ParseError> {
        if let Some(kind) = self.function(e, span)? {
            return Ok(Expr {
                kind,
                start: span.0,
                end: span.1,
            });
        }
        if let Some(expr) = self.method(e, span)? {
            return Ok(expr);
        }
        // A free function is named in full (`pl.cum_sum`); a method has a
        // receiver that isn't a name, so only the method itself is named, with
        // the leading dot that says which it is.
        let name = match &e.kind {
            PyKind::Call { callee, .. } => callee.dotted().or_else(|| match &callee.kind {
                PyKind::Attr(_, method) => Some(format!(".{method}")),
                _ => None,
            }),
            _ => None,
        };
        Err(untranslatable(
            match name {
                Some(name) => format!("`{name}()`"),
                None => "this call".to_string(),
            },
            "the language has no equivalent",
            span.0,
        ))
    }

    /// The `pl.*` and `datetime.*` free functions.
    fn function(
        &mut self,
        e: &PyExpr,
        span: (usize, usize),
    ) -> Result<Option<ExprKind>, ParseError> {
        // `pl.col("a")`, and `pl.col("a", "b")` inside a selection.
        if let Some(args) = e.as_call("pl.col") {
            let names = positional(args, span)?;
            if names.len() == 1
                && let Some(name) = names[0].as_str()
            {
                return Ok(Some(column_or_selector(name)));
            }
            let mut listed = Vec::new();
            for arg in names {
                let Some(name) = arg.as_str() else {
                    return Err(untranslatable(
                        "`pl.col()` of a computed name",
                        "a column is named by a literal",
                        arg.span.0,
                    ));
                };
                listed.push(Named {
                    name: name.to_string(),
                    start: arg.span.0,
                    end: arg.span.1,
                });
            }
            return Ok(Some(ExprKind::Columns(ColumnsSelector::List(listed))));
        }
        if let Some(args) = e.as_call("pl.lit") {
            let inner = positional(args, span)?;
            let [value] = inner[..] else {
                return Err(arity("pl.lit", 1, inner.len(), span));
            };
            return Ok(Some(self.expr(value)?.kind));
        }
        if e.as_call("pl.all").is_some() {
            return Ok(Some(ExprKind::Columns(ColumnsSelector::All)));
        }
        if e.as_call("pl.len").is_some() {
            return Ok(Some(ExprKind::Call {
                name: "ROW_COUNT".to_string(),
                args: Vec::new(),
            }));
        }
        if let Some(args) = e.as_call("pl.all_horizontal") {
            let inner = positional(args, span)?;
            let [body] = inner[..] else {
                return Err(arity("pl.all_horizontal", 1, inner.len(), span));
            };
            return Ok(Some(self.expr(body)?.kind));
        }
        if let Some(args) = e.as_call("pl.concat_list") {
            let _ = args;
            return Err(untranslatable(
                "`pl.concat_list()` outside `is_in`",
                "a list has meaning only as the right-hand side of a membership test",
                span.0,
            ));
        }
        if let Some(args) = e.as_call("pl.duration") {
            return Ok(Some(self.duration(args, span)?));
        }
        if let Some(args) = e.as_call("pl.when") {
            return Ok(Some(self.when(e, args, span)?));
        }
        if e.as_call("datetime.datetime.now").is_some() {
            return Ok(Some(ExprKind::Now));
        }
        // A temporal literal reads as the string the language writes it as, and
        // becomes a date again beside a temporal column.
        for (name, count) in [("datetime.date", 3), ("datetime.datetime", 6)] {
            if let Some(args) = e.as_call(name) {
                return Ok(Some(ExprKind::Str(temporal(name, args, count, span)?)));
            }
        }
        if let Some(args) = e.as_call("float") {
            let inner = positional(args, span)?;
            let [value] = inner[..] else {
                return Err(arity("float", 1, inner.len(), span));
            };
            return match value.as_str() {
                Some("nan") => Ok(Some(ExprKind::Number(NumLit::Float(f64::NAN)))),
                Some("inf") => Ok(Some(ExprKind::Number(NumLit::Float(f64::INFINITY)))),
                Some("-inf") => Ok(Some(ExprKind::Number(NumLit::Float(f64::NEG_INFINITY)))),
                _ => Err(untranslatable(
                    "`float()` of anything but a non-finite name",
                    "the language writes an ordinary number as a numeral",
                    span.0,
                )),
            };
        }
        Ok(None)
    }

    /// `pl.duration(unit=n)`.
    fn duration(&mut self, args: &[PyArg], span: (usize, usize)) -> Result<ExprKind, ParseError> {
        let [arg] = args else {
            return Err(untranslatable(
                "`pl.duration()` over several units",
                "the language's `interval` names one fixed-length unit",
                span.0,
            ));
        };
        let Some(unit) = arg.name.as_deref() else {
            return Err(untranslatable(
                "`pl.duration()` without a unit",
                "the language's `interval` always names its unit",
                span.0,
            ));
        };
        if !matches!(unit, "seconds" | "minutes" | "hours" | "days" | "weeks") {
            return Err(untranslatable(
                format!("`pl.duration({unit}=…)`"),
                "the language's fixed-length units are seconds, minutes, hours, days and weeks",
                arg.value.span.0,
            ));
        }
        Ok(ExprKind::Interval {
            n: Box::new(self.expr(&arg.value)?),
            unit: unit.to_string(),
            unit_start: span.0,
            unit_end: span.1,
        })
    }

    /// `pl.when(c).then(v)…[.otherwise(e)]`, which arrives inside out: the
    /// outermost call is the last `.then` or the `.otherwise`.
    fn when(
        &mut self,
        _outer: &PyExpr,
        args: &[PyArg],
        span: (usize, usize),
    ) -> Result<ExprKind, ParseError> {
        let _ = (args, span);
        Err(untranslatable(
            "`pl.when()` with no `.then()`",
            "each condition is followed by the value it gives",
            span.0,
        ))
    }

    /// The method chains: every polars expression method the target emits.
    fn method(&mut self, e: &PyExpr, span: (usize, usize)) -> Result<Option<Expr>, ParseError> {
        let build = |kind: ExprKind| Expr {
            kind,
            start: span.0,
            end: span.1,
        };
        // A conditional is a chain ending in `.then` or `.otherwise`.
        if e.as_method("then").is_some() || e.as_method("otherwise").is_some() {
            return Ok(Some(build(self.case(e, span)?)));
        }
        let simple: &[(&str, &str)] = &[
            ("str.len_chars", "LENGTH"),
            ("str.to_lowercase", "LOWER"),
            ("str.to_uppercase", "UPPER"),
            ("str.strip_chars", "TRIM"),
            ("abs", "ABS"),
            ("floor", "FLOOR"),
            ("ceil", "CEIL"),
            ("is_finite", "IS_FINITE"),
            ("is_infinite", "IS_INFINITE"),
            ("is_nan", "IS_NAN"),
            ("min", "MIN"),
            ("max", "MAX"),
            ("mean", "AVG"),
            ("count", "COUNT"),
        ];
        for (method, name) in simple {
            if let Some((receiver, args)) = self.chained(e, method) {
                if !args.is_empty() {
                    return Err(arity(method, 0, args.len(), span));
                }
                if matches!(*name, "MIN" | "MAX" | "AVG") {
                    self.notes.add(notes::NAN_COMPARISON);
                }
                return Ok(Some(build(ExprKind::Call {
                    name: name.to_string(),
                    args: vec![self.expr(receiver)?],
                })));
            }
        }
        for (method, name) in [("sum", "SUM"), ("any", "ANY"), ("all", "ALL")] {
            if let Some((receiver, args)) = self.chained(e, method) {
                if !args.is_empty() {
                    return Err(arity(method, 0, args.len(), span));
                }
                self.notes.add(notes::EMPTY_FOLD);
                if name == "SUM" {
                    self.notes.add(notes::OVERFLOW);
                }
                return Ok(Some(build(ExprKind::Call {
                    name: name.to_string(),
                    args: vec![self.expr(receiver)?],
                })));
            }
        }
        if let Some((receiver, _)) = self.chained(e, "n_unique") {
            // `n_unique` counts a null as a value, so the emitter drops them
            // first; that is exactly what `COUNT_DISTINCT` means.
            let (subject, guarded) = match receiver.as_method("drop_nulls") {
                Some((inner, _)) => (inner, true),
                None => (receiver, false),
            };
            if !guarded {
                self.notes.add(notes::COUNTS_NULL);
            }
            return Ok(Some(build(ExprKind::Call {
                name: "COUNT_DISTINCT".to_string(),
                args: vec![self.expr(subject)?],
            })));
        }
        for (method, negated) in [("is_null", false), ("is_not_null", true)] {
            if let Some((receiver, _)) = self.chained(e, method) {
                return Ok(Some(build(ExprKind::IsNull {
                    operand: Box::new(self.expr(receiver)?),
                    negated,
                })));
            }
        }
        if let Some((receiver, args)) = self.chained(e, "is_in") {
            let inner = positional(args, span)?;
            let [list] = inner[..] else {
                return Err(arity("is_in", 1, inner.len(), span));
            };
            self.notes.add(notes::NAN_COMPARISON);
            return Ok(Some(build(ExprKind::In {
                operand: Box::new(self.expr(receiver)?),
                list: self.haystack(list)?,
                negated: false,
            })));
        }
        if let Some((receiver, args)) = self.chained(e, "is_between") {
            let inner = positional(args, span)?;
            let [lo, hi] = inner[..] else {
                return Err(arity("is_between", 2, inner.len(), span));
            };
            self.notes.add(notes::NAN_COMPARISON);
            return Ok(Some(build(ExprKind::Between {
                operand: Box::new(self.expr(receiver)?),
                lo: Box::new(self.expr(lo)?),
                hi: Box::new(self.expr(hi)?),
                negated: false,
            })));
        }
        for (method, name) in [
            ("str.starts_with", "STARTS_WITH"),
            ("str.ends_with", "ENDS_WITH"),
        ] {
            if let Some((receiver, args)) = self.chained(e, method) {
                let inner = positional(args, span)?;
                let [pattern] = inner[..] else {
                    return Err(arity(method, 1, inner.len(), span));
                };
                return Ok(Some(build(ExprKind::Call {
                    name: name.to_string(),
                    args: vec![self.expr(receiver)?, self.expr(pattern)?],
                })));
            }
        }
        if let Some((receiver, args)) = self.chained(e, "str.contains") {
            let inner = positional(args, span)?;
            let [pattern] = inner[..] else {
                return Err(arity("str.contains", 1, inner.len(), span));
            };
            let Some(text) = pattern.as_str() else {
                return Err(untranslatable(
                    "`str.contains()` with a computed pattern",
                    "the pattern has to be a literal so it can be anchored",
                    pattern.span.0,
                ));
            };
            return Ok(Some(build(ExprKind::SimilarTo {
                operand: Box::new(self.expr(receiver)?),
                pattern: Box::new(Expr {
                    kind: ExprKind::Str(super::super::unanchor(text)),
                    start: pattern.span.0,
                    end: pattern.span.1,
                }),
                negated: false,
            })));
        }
        if let Some((receiver, args)) = self.chained(e, "round") {
            let inner = positional(args, span)?;
            self.notes.add(notes::ROUNDING);
            let mut call = vec![self.expr(receiver)?];
            // `round(0)` is the language's one-argument form.
            if let [digits] = inner[..]
                && !matches!(digits.kind, PyKind::Int(0))
            {
                call.push(self.expr(digits)?);
            }
            return Ok(Some(build(ExprKind::Call {
                name: "ROUND".to_string(),
                args: call,
            })));
        }
        if let Some((receiver, _)) = self.chained(e, "struct.field") {
            let _ = receiver;
            return Ok(Some(build(ExprKind::Column(self.path(e)?))));
        }
        Ok(None)
    }

    /// A method call by its (possibly dotted) name, allowing for polars'
    /// namespaces: `x.str.len_chars()` is the method `str.len_chars` on `x`.
    fn chained<'e>(&self, e: &'e PyExpr, method: &str) -> Option<(&'e PyExpr, &'e [PyArg])> {
        let PyKind::Call { callee, args } = &e.kind else {
            return None;
        };
        let mut node = callee.as_ref();
        let mut parts: Vec<&str> = Vec::new();
        while let PyKind::Attr(base, field) = &node.kind {
            parts.push(field);
            node = base;
        }
        parts.reverse();
        let wanted: Vec<&str> = method.split('.').collect();
        (parts == wanted).then_some((node, args.as_slice()))
    }

    /// A `pl.col("a").struct.field("b")` chain, as a column path.
    fn path(&mut self, e: &PyExpr) -> Result<Vec<String>, ParseError> {
        if let Some((receiver, args)) = self.chained(e, "struct.field") {
            let inner = positional(args, e.span)?;
            let [field] = inner[..] else {
                return Err(arity("struct.field", 1, inner.len(), e.span));
            };
            let Some(name) = field.as_str() else {
                return Err(untranslatable(
                    "`struct.field()` of a computed name",
                    "a field is named by a literal",
                    field.span.0,
                ));
            };
            let mut path = self.path(receiver)?;
            path.push(name.to_string());
            return Ok(path);
        }
        if let Some(args) = e.as_call("pl.col") {
            let inner = positional(args, e.span)?;
            if let [only] = inner[..]
                && let Some(name) = only.as_str()
            {
                return Ok(vec![name.to_string()]);
            }
        }
        Err(untranslatable(
            "`struct.field()` on this",
            "a field is reached inside the column that holds it",
            e.span.0,
        ))
    }

    /// The right-hand side of an `is_in`: a plain list, or a `concat_list`.
    fn haystack(&mut self, e: &PyExpr) -> Result<Vec<Expr>, ParseError> {
        let items = match &e.kind {
            PyKind::List(items) => items,
            _ => match e.as_call("pl.concat_list") {
                Some(args) => {
                    let inner = positional(args, e.span)?;
                    let [list] = inner[..] else {
                        return Err(arity("pl.concat_list", 1, inner.len(), e.span));
                    };
                    match &list.kind {
                        PyKind::List(items) => items,
                        _ => {
                            return Err(untranslatable(
                                "`pl.concat_list()` of anything but a list",
                                "the language's `IN` takes a written-out list",
                                list.span.0,
                            ));
                        }
                    }
                }
                None => {
                    return Err(untranslatable(
                        "`is_in()` over a computed set",
                        "the language's `IN` takes a written-out list",
                        e.span.0,
                    ));
                }
            },
        };
        items.iter().map(|item| self.expr(item)).collect()
    }

    /// A `pl.when(…).then(…)` chain, read from the outside in and reversed.
    fn case(&mut self, e: &PyExpr, span: (usize, usize)) -> Result<ExprKind, ParseError> {
        let mut els = None;
        let mut node = e;
        if let Some((receiver, args)) = e.as_method("otherwise") {
            let inner = positional(args, span)?;
            let [value] = inner[..] else {
                return Err(arity("otherwise", 1, inner.len(), span));
            };
            els = Some(Box::new(self.expr(value)?));
            node = receiver;
        }
        let mut whens: Vec<(Expr, Expr)> = Vec::new();
        loop {
            let Some((before_then, then_args)) = node.as_method("then") else {
                return Err(untranslatable(
                    "a `pl.when()` chain in this form",
                    "each `when` is followed by the `then` that gives its value",
                    node.span.0,
                ));
            };
            let then_inner = positional(then_args, span)?;
            let [value] = then_inner[..] else {
                return Err(arity("then", 1, then_inner.len(), span));
            };
            // The `when` is either `pl.when(c)` or `….when(c)`.
            let (receiver, when_args) = match before_then.as_call("pl.when") {
                Some(args) => (None, args),
                None => match before_then.as_method("when") {
                    Some((inner, args)) => (Some(inner), args),
                    None => {
                        return Err(untranslatable(
                            "a `.then()` with no `when`",
                            "each value follows the condition that selects it",
                            before_then.span.0,
                        ));
                    }
                },
            };
            let when_inner = positional(when_args, span)?;
            let [cond] = when_inner[..] else {
                return Err(arity("when", 1, when_inner.len(), span));
            };
            whens.push((self.expr(cond)?, self.expr(value)?));
            match receiver {
                Some(inner) => node = inner,
                None => break,
            }
        }
        whens.reverse();
        Ok(ExprKind::Case { whens, els })
    }
}

/// `pl.col("x")` is a column — unless its name is a regular expression, which
/// is how polars spells a selection.
fn column_or_selector(name: &str) -> ExprKind {
    // polars anchors a column pattern, and the target wraps the language's
    // unanchored one; both anchors come back off.
    if let Some(inner) = name.strip_prefix("^").and_then(|r| r.strip_suffix("$")) {
        let pattern = inner
            .strip_prefix(".*(?:")
            .and_then(|r| r.strip_suffix(").*"))
            .unwrap_or(inner);
        return ExprKind::Columns(ColumnsSelector::Regex {
            pattern: pattern.to_string(),
            start: 0,
            end: 0,
        });
    }
    ExprKind::Column(vec![name.to_string()])
}

/// The cast an emitter puts before a duration is added to a date. The language
/// promotes on its own, so the cast carries no meaning of its own here.
fn strip_promotion(e: &PyExpr) -> &PyExpr {
    match e.as_method("cast") {
        Some((receiver, _)) => receiver,
        None => e,
    }
}

/// `datetime.date(y, m, d)` and `datetime.datetime(y, m, d, h, mi, s)` as the
/// ISO 8601 string the language writes a temporal literal as.
fn temporal(
    name: &str,
    args: &[PyArg],
    count: usize,
    span: (usize, usize),
) -> Result<String, ParseError> {
    let inner = positional(args, span)?;
    let mut parts = Vec::new();
    for arg in &inner {
        match arg.kind {
            PyKind::Int(n) => parts.push(n),
            _ => {
                return Err(untranslatable(
                    format!("`{name}()` of a computed value"),
                    "the language writes a date as a literal string, so only a literal converts",
                    arg.span.0,
                ));
            }
        }
    }
    // `datetime.datetime` takes its time parts optionally.
    while parts.len() < count {
        parts.push(0);
    }
    if parts.len() != count {
        return Err(arity(name, count, inner.len(), span));
    }
    Ok(if count == 3 {
        format!("{:04}-{:02}-{:02}", parts[0], parts[1], parts[2])
    } else {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]
        )
    })
}

fn positional(args: &[PyArg], span: (usize, usize)) -> Result<Vec<&PyExpr>, ParseError> {
    if let Some(named) = args.iter().find(|a| a.name.is_some()) {
        let name = named.name.clone().unwrap_or_default();
        let _ = span;
        return Err(untranslatable(
            format!("`{name}=`"),
            "the language's functions take their arguments in order",
            named.value.span.0,
        ));
    }
    Ok(args.iter().map(|a| &a.value).collect())
}

fn arity(name: &str, want: usize, got: usize, span: (usize, usize)) -> ParseError {
    ParseError {
        message: format!("`{name}()` takes {want} arguments, but {got} were given"),
        at: span.0,
    }
}
