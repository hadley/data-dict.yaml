//! The typed IR an assertion becomes once it has been checked: the single
//! contract the interpreter and every code emitter read.
//!
//! Where the [surface AST](super::Expr) mirrors what was written — a function
//! is a name, a literal is a span, nothing carries a type — the IR carries what
//! a backend needs and nothing about how it was spelled. Every node knows its
//! type, column references are resolved, function names have become one
//! [`Op`] variant each, and a temporal string literal has become a real date.
//!
//! [`lower`] is only defined for an expression that passed every check in
//! [`super::check`], so nothing here reports diagnostics or handles ill-typed
//! input; see `site/expression-execution.md`.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};

use super::{
    ArithOp, AssertExpr, CheckEnv, CmpOp, ColumnsSelector, Expr, ExprKind, NumLit, Shape, SigShape,
    Ty, kind_to_ty, signature,
};

/// A checked assertion, ready to evaluate or translate.
#[derive(Debug, Clone)]
pub struct TypedAssertion {
    /// The expression's one `COLUMNS(...)`, if it has one. The selection stays
    /// symbolic — `root` holds [`NodeKind::Selected`] where it appeared — so a
    /// backend may either instantiate `root` once per resolved column and
    /// combine the results with `AND`, or use a multi-column idiom of its own.
    pub selection: Option<Selection>,
    pub root: TypedExpr,
}

impl TypedAssertion {
    /// Every column the expression reads, in the order it first names them,
    /// including the ones a `COLUMNS(...)` resolved to. A caller knows from
    /// this which columns to select or load before evaluating the expression.
    pub fn columns(&self) -> Vec<ColumnRef> {
        let mut out = Vec::new();
        collect(&self.root, &mut out);
        if let Some(selection) = &self.selection {
            for column in &selection.columns {
                push_once(&mut out, column.clone());
            }
        }
        return out;

        fn collect(e: &TypedExpr, out: &mut Vec<ColumnRef>) {
            if let NodeKind::Column(c) = &e.kind {
                push_once(out, c.clone());
            }
            for child in e.children() {
                collect(child, out);
            }
        }

        fn push_once(out: &mut Vec<ColumnRef>, column: ColumnRef) {
            if !out.iter().any(|c| c.path == column.path) {
                out.push(column);
            }
        }
    }
}

/// A resolved `COLUMNS(...)`: how it was written, and what it picked out.
#[derive(Debug, Clone)]
pub struct Selection {
    pub form: SelectorForm,
    pub columns: Vec<ColumnRef>,
}

#[derive(Debug, Clone)]
pub enum SelectorForm {
    All,
    /// The regex as written, matched unanchored. A target whose own regexes are
    /// anchored has to wrap it.
    Regex(String),
    List,
}

#[derive(Debug, Clone)]
pub struct TypedExpr {
    pub kind: NodeKind,
    pub ty: Type,
    pub shape: Shape,
    /// Byte offsets in the assertion text, for diagnostics that point into it.
    pub span: (usize, usize),
}

/// A node's type. The language's six types plus [`Any`](Type::Any) for the
/// three things that genuinely have none: the `NULL` literal, a `COLUMNS(...)`
/// standing for columns that need not agree, and a `CASE` whose branches
/// disagree.
///
/// `Number` does not say integer or float. A literal's representation is in its
/// node ([`NodeKind::Int`] vs [`NodeKind::Float`]), but a column's is a property
/// of the data, not of the dictionary, so it isn't known until the data is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Number,
    String,
    Bool,
    Date,
    Datetime,
    Interval,
    Any,
}

impl TypedExpr {
    /// This node's operands, for a walk over the tree.
    pub fn children(&self) -> Vec<&TypedExpr> {
        match &self.kind {
            NodeKind::Int(_)
            | NodeKind::Float(_)
            | NodeKind::Str(_)
            | NodeKind::Bool(_)
            | NodeKind::Null
            | NodeKind::Date(_)
            | NodeKind::Datetime(_)
            | NodeKind::Column(_)
            | NodeKind::Selected
            | NodeKind::Now => Vec::new(),
            NodeKind::Neg(x) | NodeKind::Not(x) => vec![x],
            NodeKind::Arith { lhs, rhs, .. } | NodeKind::Compare { lhs, rhs, .. } => vec![lhs, rhs],
            NodeKind::And(l, r) | NodeKind::Or(l, r) => vec![l, r],
            NodeKind::IsNull { operand, .. } => vec![operand],
            NodeKind::Between {
                operand, lo, hi, ..
            } => vec![operand, lo, hi],
            NodeKind::In { operand, list, .. } => {
                let mut out = vec![operand.as_ref()];
                out.extend(list.iter());
                out
            }
            NodeKind::Like {
                operand, pattern, ..
            } => match pattern {
                LikePattern::Dynamic(p) => vec![operand, p],
                _ => vec![operand],
            },
            NodeKind::SimilarTo {
                operand, pattern, ..
            } => vec![operand, pattern],
            NodeKind::Func { args, .. } => args.iter().collect(),
            NodeKind::Interval { n, .. } => vec![n],
            NodeKind::Case { whens, els } => {
                let mut out = Vec::new();
                for (c, r) in whens {
                    out.push(c);
                    out.push(r);
                }
                out.extend(els.as_deref());
                out
            }
        }
    }
}

impl Type {
    /// The language's name for this type, for reporting.
    pub fn name(self) -> &'static str {
        match self {
            Type::Number => "number",
            Type::String => "string",
            Type::Bool => "boolean",
            Type::Date => "date",
            Type::Datetime => "datetime",
            Type::Interval => "interval",
            Type::Any => "any",
        }
    }
}

/// A column or struct field, with the type the dictionary declares for it.
#[derive(Debug, Clone)]
pub struct ColumnRef {
    /// The column name, then one field name per dot: `address.zip`.
    pub path: Vec<String>,
    pub ty: Type,
}

/// A datetime literal, in whichever of the two spellings the language accepts.
#[derive(Debug, Clone, Copy)]
pub enum DatetimeConst {
    Offset(DateTime<FixedOffset>),
    Naive(NaiveDateTime),
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    /// A string literal that met a `date` and parsed as one.
    Date(NaiveDate),
    /// A string literal that met a `datetime` and parsed as one.
    Datetime(DatetimeConst),
    Column(ColumnRef),
    /// Stands for the column a [`Selection`] currently supplies.
    Selected,
    Neg(Box<TypedExpr>),
    Not(Box<TypedExpr>),
    Arith {
        op: ArithOp,
        lhs: Box<TypedExpr>,
        rhs: Box<TypedExpr>,
    },
    Compare {
        op: CmpOp,
        lhs: Box<TypedExpr>,
        rhs: Box<TypedExpr>,
    },
    And(Box<TypedExpr>, Box<TypedExpr>),
    Or(Box<TypedExpr>, Box<TypedExpr>),
    IsNull {
        operand: Box<TypedExpr>,
        negated: bool,
    },
    Between {
        operand: Box<TypedExpr>,
        lo: Box<TypedExpr>,
        hi: Box<TypedExpr>,
        negated: bool,
    },
    In {
        operand: Box<TypedExpr>,
        list: Vec<TypedExpr>,
        negated: bool,
    },
    Like {
        operand: Box<TypedExpr>,
        pattern: LikePattern,
        negated: bool,
    },
    SimilarTo {
        operand: Box<TypedExpr>,
        pattern: Box<TypedExpr>,
        negated: bool,
    },
    Func {
        op: Op,
        args: Vec<TypedExpr>,
    },
    Now,
    Interval {
        n: Box<TypedExpr>,
        unit: IntervalUnit,
    },
    Case {
        whens: Vec<(TypedExpr, TypedExpr)>,
        els: Option<Box<TypedExpr>>,
    },
}

/// A `LIKE` pattern, taken apart once here so every backend agrees about it.
/// The three special shapes are worth keeping because most targets spell them
/// with a dedicated function that is clearer than a regex.
#[derive(Debug, Clone)]
pub enum LikePattern {
    /// `'abc'` — no wildcards, so plain equality.
    Exact(String),
    /// `'abc%'`
    Prefix(String),
    /// `'%abc'`
    Suffix(String),
    /// Anything else, as an anchored regex.
    Regex(String),
    /// A computed pattern. A target that must compile the pattern at
    /// translation time can't support this one.
    Dynamic(Box<TypedExpr>),
}

/// One variant per function in the language. Backends match on this; the
/// spelling that produced it, and its case, are gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Length,
    Lower,
    Upper,
    Trim,
    StartsWith,
    EndsWith,
    Abs,
    Floor,
    Ceil,
    Round,
    Mod,
    Min,
    Max,
    Sum,
    Avg,
    Count,
    RowCount,
    CountDistinct,
    Any,
    All,
}

impl Op {
    fn from_name(lowercased: &str) -> Option<Op> {
        Some(match lowercased {
            "length" => Op::Length,
            "lower" => Op::Lower,
            "upper" => Op::Upper,
            "trim" => Op::Trim,
            "starts_with" => Op::StartsWith,
            "ends_with" => Op::EndsWith,
            "abs" => Op::Abs,
            "floor" => Op::Floor,
            "ceil" => Op::Ceil,
            "round" => Op::Round,
            "mod" => Op::Mod,
            "min" => Op::Min,
            "max" => Op::Max,
            "sum" => Op::Sum,
            "avg" => Op::Avg,
            "count" => Op::Count,
            "row_count" => Op::RowCount,
            "count_distinct" => Op::CountDistinct,
            "any" => Op::Any,
            "all" => Op::All,
            _ => return None,
        })
    }

    /// Whether this folds a column into one value, which decides the node's
    /// [`Shape`] and, for the whole expression, whether a violation can name a
    /// row at all.
    pub fn is_aggregate(self) -> bool {
        matches!(
            self,
            Op::Min
                | Op::Max
                | Op::Sum
                | Op::Avg
                | Op::Count
                | Op::RowCount
                | Op::CountDistinct
                | Op::Any
                | Op::All
        )
    }
}

/// The five fixed-length interval units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalUnit {
    Seconds,
    Minutes,
    Hours,
    Days,
    Weeks,
}

impl IntervalUnit {
    fn from_name(lowercased: &str) -> Option<IntervalUnit> {
        Some(match lowercased {
            "seconds" => IntervalUnit::Seconds,
            "minutes" => IntervalUnit::Minutes,
            "hours" => IntervalUnit::Hours,
            "days" => IntervalUnit::Days,
            "weeks" => IntervalUnit::Weeks,
            _ => return None,
        })
    }

    /// How many seconds one of these lasts. All five are fixed-length, which is
    /// why the calendar units are excluded from the language.
    pub fn seconds(self) -> i64 {
        match self {
            IntervalUnit::Seconds => 1,
            IntervalUnit::Minutes => 60,
            IntervalUnit::Hours => 3600,
            IntervalUnit::Days => 86_400,
            IntervalUnit::Weeks => 604_800,
        }
    }
}

fn to_type(ty: Ty) -> Type {
    match ty {
        Ty::Number => Type::Number,
        Ty::String => Type::String,
        Ty::Bool => Type::Bool,
        Ty::Date => Type::Date,
        Ty::Datetime => Type::Datetime,
        Ty::Interval => Type::Interval,
        // A struct, a list, or an untyped column can only reach here through an
        // expression that failed to check, which `lower` is not defined for.
        Ty::Struct | Ty::List | Ty::Any | Ty::Unknown => Type::Any,
    }
}

/// Lower a checked assertion to its IR.
///
/// Only defined for an expression that [`super::check`] reported no errors for:
/// it assumes every column resolves, every operand fits, and there is at most
/// one `COLUMNS(...)`. `None` means the expression was not in fact checked, or
/// the environment has changed under it.
pub fn lower(expr: &AssertExpr, env: &dyn CheckEnv) -> Option<TypedAssertion> {
    let mut cx = Lowerer {
        env,
        selection: None,
    };
    let root = cx.expr(&expr.root)?;
    Some(TypedAssertion {
        selection: cx.selection,
        root,
    })
}

struct Lowerer<'a> {
    env: &'a dyn CheckEnv,
    selection: Option<Selection>,
}

impl Lowerer<'_> {
    fn expr(&mut self, e: &Expr) -> Option<TypedExpr> {
        let span = (e.start, e.end);
        let node = match &e.kind {
            ExprKind::Number(NumLit::Int(n)) => lit(NodeKind::Int(*n), Type::Number, span),
            ExprKind::Number(NumLit::Float(x)) => lit(NodeKind::Float(*x), Type::Number, span),
            ExprKind::Str(s) => lit(NodeKind::Str(s.clone()), Type::String, span),
            ExprKind::Bool(b) => lit(NodeKind::Bool(*b), Type::Bool, span),
            ExprKind::Null => lit(NodeKind::Null, Type::Any, span),
            ExprKind::Column(path) => {
                let column = self.column_ref(path)?;
                TypedExpr {
                    ty: column.ty,
                    kind: NodeKind::Column(column),
                    shape: Shape::Row,
                    span,
                }
            }
            ExprKind::Columns(sel) => {
                self.resolve_selection(sel)?;
                TypedExpr {
                    kind: NodeKind::Selected,
                    ty: Type::Any,
                    shape: Shape::Row,
                    span,
                }
            }
            ExprKind::Neg(inner) => {
                let inner = self.boxed(inner)?;
                TypedExpr {
                    ty: Type::Number,
                    shape: inner.shape,
                    kind: NodeKind::Neg(inner),
                    span,
                }
            }
            ExprKind::Not(inner) => {
                let inner = self.boxed(inner)?;
                TypedExpr {
                    ty: Type::Bool,
                    shape: inner.shape,
                    kind: NodeKind::Not(inner),
                    span,
                }
            }
            ExprKind::And(l, r) => {
                let (l, r) = (self.boxed(l)?, self.boxed(r)?);
                TypedExpr {
                    ty: Type::Bool,
                    shape: l.shape.max(r.shape),
                    kind: NodeKind::And(l, r),
                    span,
                }
            }
            ExprKind::Or(l, r) => {
                let (l, r) = (self.boxed(l)?, self.boxed(r)?);
                TypedExpr {
                    ty: Type::Bool,
                    shape: l.shape.max(r.shape),
                    kind: NodeKind::Or(l, r),
                    span,
                }
            }
            ExprKind::Arith { op, lhs, rhs } => {
                let (lhs, rhs) = (self.boxed(lhs)?, self.boxed(rhs)?);
                // A shifted date or datetime is a datetime; everything else
                // here is numeric.
                let temporal = matches!(lhs.ty, Type::Date | Type::Datetime)
                    || matches!(rhs.ty, Type::Date | Type::Datetime);
                let interval = lhs.ty == Type::Interval || rhs.ty == Type::Interval;
                TypedExpr {
                    ty: if temporal && interval {
                        Type::Datetime
                    } else {
                        Type::Number
                    },
                    shape: lhs.shape.max(rhs.shape),
                    kind: NodeKind::Arith { op: *op, lhs, rhs },
                    span,
                }
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let (mut lhs, mut rhs) = (self.boxed(lhs)?, self.boxed(rhs)?);
                self.coerce_temporal(&mut lhs, &mut rhs);
                TypedExpr {
                    ty: Type::Bool,
                    shape: lhs.shape.max(rhs.shape),
                    kind: NodeKind::Compare { op: *op, lhs, rhs },
                    span,
                }
            }
            ExprKind::IsNull { operand, negated } => {
                let operand = self.boxed(operand)?;
                TypedExpr {
                    ty: Type::Bool,
                    shape: operand.shape,
                    kind: NodeKind::IsNull {
                        operand,
                        negated: *negated,
                    },
                    span,
                }
            }
            ExprKind::Between {
                operand,
                lo,
                hi,
                negated,
            } => {
                let mut operand = self.boxed(operand)?;
                let (mut lo, mut hi) = (self.boxed(lo)?, self.boxed(hi)?);
                self.coerce_temporal(&mut operand, &mut lo);
                self.coerce_temporal(&mut operand, &mut hi);
                TypedExpr {
                    ty: Type::Bool,
                    shape: operand.shape.max(lo.shape).max(hi.shape),
                    kind: NodeKind::Between {
                        operand,
                        lo,
                        hi,
                        negated: *negated,
                    },
                    span,
                }
            }
            ExprKind::In {
                operand,
                list,
                negated,
            } => {
                let mut operand = self.boxed(operand)?;
                let mut shape = operand.shape;
                let mut items = Vec::with_capacity(list.len());
                for item in list {
                    let mut item = self.expr(item)?;
                    self.coerce_to(&mut item, operand.ty);
                    shape = shape.max(item.shape);
                    items.push(item);
                }
                // The operand may be the literal instead, against a temporal
                // list: `'2000-01-01' IN (start_date, end_date)`.
                if let Some(ty) = items.first().map(|i| i.ty) {
                    self.coerce_to(&mut operand, ty);
                }
                TypedExpr {
                    ty: Type::Bool,
                    shape,
                    kind: NodeKind::In {
                        operand,
                        list: items,
                        negated: *negated,
                    },
                    span,
                }
            }
            ExprKind::Like {
                operand,
                pattern,
                negated,
            } => {
                let operand = self.boxed(operand)?;
                let pattern = match &pattern.kind {
                    ExprKind::Str(p) => like_pattern(p),
                    _ => LikePattern::Dynamic(self.boxed(pattern)?),
                };
                TypedExpr {
                    ty: Type::Bool,
                    shape: operand.shape,
                    kind: NodeKind::Like {
                        operand,
                        pattern,
                        negated: *negated,
                    },
                    span,
                }
            }
            ExprKind::SimilarTo {
                operand,
                pattern,
                negated,
            } => {
                let (operand, pattern) = (self.boxed(operand)?, self.boxed(pattern)?);
                TypedExpr {
                    ty: Type::Bool,
                    shape: operand.shape.max(pattern.shape),
                    kind: NodeKind::SimilarTo {
                        operand,
                        pattern,
                        negated: *negated,
                    },
                    span,
                }
            }
            ExprKind::Now => lit(NodeKind::Now, Type::Datetime, span),
            ExprKind::Interval { n, unit, .. } => {
                let n = self.boxed(n)?;
                TypedExpr {
                    ty: Type::Interval,
                    shape: n.shape,
                    kind: NodeKind::Interval {
                        n,
                        unit: IntervalUnit::from_name(&unit.to_ascii_lowercase())?,
                    },
                    span,
                }
            }
            ExprKind::Call { name, args } => self.call(name, args, span)?,
            ExprKind::Case { whens, els } => {
                let mut shape = Shape::Const;
                let mut lowered = Vec::with_capacity(whens.len());
                for (cond, result) in whens {
                    let cond = self.expr(cond)?;
                    let result = self.expr(result)?;
                    shape = shape.max(cond.shape).max(result.shape);
                    lowered.push((cond, result));
                }
                let els = match els {
                    Some(e) => {
                        let e = self.boxed(e)?;
                        shape = shape.max(e.shape);
                        Some(e)
                    }
                    None => None,
                };
                TypedExpr {
                    ty: case_type(&lowered, els.as_deref()),
                    shape,
                    kind: NodeKind::Case {
                        whens: lowered,
                        els,
                    },
                    span,
                }
            }
        };
        Some(node)
    }

    fn boxed(&mut self, e: &Expr) -> Option<Box<TypedExpr>> {
        self.expr(e).map(Box::new)
    }

    /// Turn a string literal that met a temporal operand into a real date or
    /// datetime, so a backend constructs one natively rather than leaning on
    /// the target's own string coercion. This is the comparability rule that
    /// admits `birthdate >= '2000-01-01'`, applied to the value.
    fn coerce_temporal(&self, a: &mut TypedExpr, b: &mut TypedExpr) {
        let (a_ty, b_ty) = (a.ty, b.ty);
        self.coerce_to(a, b_ty);
        self.coerce_to(b, a_ty);
    }

    fn coerce_to(&self, lit: &mut TypedExpr, target: Type) {
        let NodeKind::Str(s) = &lit.kind else { return };
        match target {
            Type::Date => {
                if let Some(d) = self.env.as_date(s) {
                    lit.kind = NodeKind::Date(d);
                    lit.ty = Type::Date;
                }
            }
            Type::Datetime => {
                if let Some(t) = self.env.as_datetime(s) {
                    lit.kind = NodeKind::Datetime(t);
                    lit.ty = Type::Datetime;
                }
            }
            _ => {}
        }
    }

    fn call(&mut self, name: &str, args: &[Expr], span: (usize, usize)) -> Option<TypedExpr> {
        let lowered_name = name.to_ascii_lowercase();
        let op = Op::from_name(&lowered_name)?;
        let sig = signature(&lowered_name)?;
        let mut shape = Shape::Const;
        let mut lowered = Vec::with_capacity(args.len());
        for a in args {
            let a = self.expr(a)?;
            shape = shape.max(a.shape);
            lowered.push(a);
        }
        let ty = match sig.ret {
            super::Ret::Fixed(t) => to_type(t),
            // `MIN`/`MAX` return their argument's type.
            super::Ret::SameAsArg => lowered.first().map_or(Type::Any, |a| a.ty),
        };
        Some(TypedExpr {
            kind: NodeKind::Func { op, args: lowered },
            ty,
            shape: if sig.shape == SigShape::Aggregate {
                Shape::Agg
            } else {
                shape
            },
            span,
        })
    }

    fn column_ref(&self, path: &[String]) -> Option<ColumnRef> {
        let kind = if path.len() == 1 {
            self.env.column(&path[0])?
        } else {
            self.env.field(path)?
        };
        Some(ColumnRef {
            path: path.to_vec(),
            ty: to_type(kind_to_ty(kind)),
        })
    }

    /// Record the expression's one `COLUMNS(...)` and the columns it picked out.
    fn resolve_selection(&mut self, sel: &ColumnsSelector) -> Option<()> {
        let (form, columns) = match sel {
            ColumnsSelector::All => (SelectorForm::All, self.env.columns()),
            ColumnsSelector::Regex { pattern, .. } => {
                let re = regex::Regex::new(pattern).ok()?;
                let matched = self
                    .env
                    .columns()
                    .into_iter()
                    .filter(|(n, _)| re.is_match(n))
                    .collect();
                (SelectorForm::Regex(pattern.clone()), matched)
            }
            ColumnsSelector::List(names) => {
                let mut matched = Vec::with_capacity(names.len());
                for n in names {
                    matched.push((n.name.clone(), self.env.column(&n.name)?));
                }
                (SelectorForm::List, matched)
            }
        };
        self.selection = Some(Selection {
            form,
            columns: columns
                .into_iter()
                .map(|(name, kind)| ColumnRef {
                    path: vec![name],
                    ty: to_type(kind_to_ty(kind)),
                })
                .collect(),
        });
        Some(())
    }
}

fn lit(kind: NodeKind, ty: Type, span: (usize, usize)) -> TypedExpr {
    TypedExpr {
        kind,
        ty,
        shape: Shape::Const,
        span,
    }
}

/// A `CASE`'s type is its branches' common type, or [`Type::Any`] when they
/// disagree — which the language permits, though it rarely helps.
fn case_type(whens: &[(TypedExpr, TypedExpr)], els: Option<&TypedExpr>) -> Type {
    let mut result: Option<Type> = None;
    for t in whens.iter().map(|(_, r)| r.ty).chain(els.map(|e| e.ty)) {
        result = Some(match result {
            None => t,
            Some(prev) if prev == t || t == Type::Any => prev,
            Some(Type::Any) => t,
            Some(_) => Type::Any,
        });
    }
    result.unwrap_or(Type::Any)
}

/// Turn a string literal that met a temporal operand into a real date or
/// datetime, so a backend constructs one natively rather than relying on the
/// target's own string coercion. This is the comparability rule that admits
/// `birthdate >= '2000-01-01'`, applied to the value rather than the type.
/// Take a literal `LIKE` pattern apart. `%` matches any run of characters and
/// `_` any one; the language gives them no escape, so every other character is
/// literal.
fn like_pattern(pattern: &str) -> LikePattern {
    let wildcards = pattern.matches(['%', '_']).count();
    if wildcards == 0 {
        return LikePattern::Exact(pattern.to_string());
    }
    if wildcards == 1 {
        if let Some(prefix) = pattern.strip_suffix('%') {
            return LikePattern::Prefix(prefix.to_string());
        }
        if let Some(suffix) = pattern.strip_prefix('%') {
            return LikePattern::Suffix(suffix.to_string());
        }
    }
    let mut re = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '%' => re.push_str(".*"),
            '_' => re.push('.'),
            other => re.push_str(&regex::escape(&other.to_string())),
        }
    }
    re.push('$');
    LikePattern::Regex(re)
}

#[cfg(test)]
mod tests {
    use super::super::tests::TestEnv;
    use super::*;
    use crate::assert_expr::AssertExpr;

    fn ir(s: &str) -> TypedAssertion {
        let expr = AssertExpr::parse(s).unwrap_or_else(|e| panic!("parse({s:?}): {}", e.message));
        let findings = super::super::check(&expr, &TestEnv);
        assert!(
            findings.is_empty(),
            "{s:?} should check clean: {findings:?}"
        );
        lower(&expr, &TestEnv).unwrap_or_else(|| panic!("lower({s:?}) returned None"))
    }

    /// A one-line rendering of the tree, so a test can assert on its shape
    /// without matching a dozen nested patterns.
    fn render(e: &TypedExpr) -> String {
        let ty = format!("{:?}", e.ty).to_lowercase();
        let inner = match &e.kind {
            NodeKind::Int(n) => format!("{n}"),
            NodeKind::Float(x) => format!("{x}f"),
            NodeKind::Str(s) => format!("{s:?}"),
            NodeKind::Bool(b) => format!("{b}"),
            NodeKind::Null => "null".to_string(),
            NodeKind::Date(d) => format!("date({d})"),
            NodeKind::Datetime(t) => match t {
                DatetimeConst::Offset(t) => format!("datetime({})", t.to_rfc3339()),
                DatetimeConst::Naive(t) => format!("datetime({t})"),
            },
            NodeKind::Column(c) => format!("col({})", c.path.join(".")),
            NodeKind::Selected => "selected".to_string(),
            NodeKind::Neg(x) => format!("neg({})", render(x)),
            NodeKind::Not(x) => format!("not({})", render(x)),
            NodeKind::Arith { op, lhs, rhs } => {
                format!("{op:?}({}, {})", render(lhs), render(rhs))
            }
            NodeKind::Compare { op, lhs, rhs } => {
                format!("{op:?}({}, {})", render(lhs), render(rhs))
            }
            NodeKind::And(l, r) => format!("and({}, {})", render(l), render(r)),
            NodeKind::Or(l, r) => format!("or({}, {})", render(l), render(r)),
            NodeKind::IsNull { operand, negated } => {
                format!("isnull{}({})", neg(*negated), render(operand))
            }
            NodeKind::Between {
                operand,
                lo,
                hi,
                negated,
            } => format!(
                "between{}({}, {}, {})",
                neg(*negated),
                render(operand),
                render(lo),
                render(hi)
            ),
            NodeKind::In {
                operand,
                list,
                negated,
            } => {
                let items: Vec<String> = list.iter().map(render).collect();
                format!(
                    "in{}({}, [{}])",
                    neg(*negated),
                    render(operand),
                    items.join(", ")
                )
            }
            NodeKind::Like {
                operand,
                pattern,
                negated,
            } => format!(
                "like{}({}, {})",
                neg(*negated),
                render(operand),
                match pattern {
                    LikePattern::Exact(p) => format!("exact {p:?}"),
                    LikePattern::Prefix(p) => format!("prefix {p:?}"),
                    LikePattern::Suffix(p) => format!("suffix {p:?}"),
                    LikePattern::Regex(p) => format!("regex {p:?}"),
                    LikePattern::Dynamic(e) => format!("dynamic {}", render(e)),
                }
            ),
            NodeKind::SimilarTo {
                operand,
                pattern,
                negated,
            } => format!(
                "similar{}({}, {})",
                neg(*negated),
                render(operand),
                render(pattern)
            ),
            NodeKind::Func { op, args } => {
                let args: Vec<String> = args.iter().map(render).collect();
                format!("{op:?}({})", args.join(", "))
            }
            NodeKind::Now => "now".to_string(),
            NodeKind::Interval { n, unit } => format!("interval({}, {unit:?})", render(n)),
            NodeKind::Case { whens, els } => {
                let mut parts: Vec<String> = whens
                    .iter()
                    .map(|(c, r)| format!("when {} then {}", render(c), render(r)))
                    .collect();
                if let Some(e) = els {
                    parts.push(format!("else {}", render(e)));
                }
                format!("case({})", parts.join(", "))
            }
        };
        format!("{inner}:{ty}")
    }

    fn neg(negated: bool) -> &'static str {
        if negated { "!" } else { "" }
    }

    fn rendered(s: &str) -> String {
        render(&ir(s).root)
    }

    #[test]
    fn every_node_carries_its_type() {
        assert_eq!(rendered("qty > 0"), "Gt(col(qty):number, 0:number):bool");
        assert_eq!(
            rendered("LENGTH(postcode) <= 10"),
            "Le(Length(col(postcode):string):number, 10:number):bool"
        );
        assert_eq!(rendered("flag"), "col(flag):bool");
        assert_eq!(
            rendered("qty / 2 > 0.5"),
            "Gt(Div(col(qty):number, 2:number):number, 0.5f:number):bool"
        );
    }

    #[test]
    fn a_function_name_becomes_one_op() {
        // The spelling and its case are gone by now.
        for spelling in ["LENGTH(s)", "length(s)", "LeNgTh(s)"] {
            let expr = format!("{spelling} > 0");
            assert!(rendered(&expr).starts_with("Gt(Length("), "{expr}");
        }
    }

    #[test]
    fn a_temporal_string_literal_becomes_a_constant() {
        assert_eq!(
            rendered("d >= '2000-01-01'"),
            "Ge(col(d):date, date(2000-01-01):date):bool"
        );
        // Either side, and through BETWEEN and IN too.
        assert_eq!(
            rendered("'2000-01-01' <= d"),
            "Le(date(2000-01-01):date, col(d):date):bool"
        );
        assert!(rendered("d BETWEEN '2000-01-01' AND '2030-01-01'").contains("date(2030-01-01)"));
        assert!(rendered("d IN ('2000-01-01')").contains("date(2000-01-01)"));
    }

    #[test]
    fn a_string_that_is_not_a_date_stays_a_string() {
        assert_eq!(
            rendered("s = 'not a date'"),
            "Eq(col(s):string, \"not a date\":string):bool"
        );
    }

    #[test]
    fn a_datetime_literal_keeps_its_spelling() {
        assert!(
            rendered("ts >= '2024-01-31T09:30:00Z'")
                .contains("datetime(2024-01-31T09:30:00+00:00)")
        );
        assert!(rendered("ts >= '2024-01-31T09:30:00'").contains("datetime(2024-01-31 09:30:00)"));
    }

    #[test]
    fn like_patterns_are_taken_apart() {
        assert!(rendered("s LIKE 'NZ-%'").contains("prefix \"NZ-\""));
        assert!(rendered("s LIKE '%.nz'").contains("suffix \".nz\""));
        assert!(rendered("s LIKE 'exact'").contains("exact \"exact\""));
        assert!(rendered("s LIKE 'a%b_c'").contains(r#"regex "^a.*b.c$""#));
        // A wildcard in the middle needs the general form, and a regex
        // metacharacter in the pattern is literal, so it is escaped.
        assert!(rendered("s LIKE 'a.b%c'").contains(r#"regex "^a\\.b.*c$""#));
        // A computed pattern can't be decomposed.
        assert!(rendered("s LIKE LOWER(postcode)").contains("dynamic"));
    }

    #[test]
    fn shapes_come_out_of_the_walk() {
        assert_eq!(ir("qty > 0").root.shape, Shape::Row);
        assert_eq!(ir("SUM(qty) > 0").root.shape, Shape::Agg);
        assert_eq!(ir("ROW_COUNT() > 0").root.shape, Shape::Agg);
        // Mixed grain: a row-level operand widens the whole thing back to row.
        assert_eq!(ir("qty <= 2 * MIN(qty)").root.shape, Shape::Row);
        assert_eq!(ir("1 > 0").root.shape, Shape::Const);
    }

    #[test]
    fn min_returns_its_argument_type() {
        assert_eq!(
            rendered("d >= MIN(d)"),
            "Ge(col(d):date, Min(col(d):date):date):bool"
        );
    }

    #[test]
    fn a_shifted_date_is_a_datetime() {
        assert_eq!(
            rendered("d + interval(12, hours) < NOW()"),
            "Lt(Add(col(d):date, interval(12:number, Hours):interval):datetime, now:datetime):bool"
        );
    }

    #[test]
    fn columns_stays_symbolic_with_its_resolution() {
        let ir = ir("COLUMNS('q[34]') IS NOT NULL");
        let selection = ir.selection.expect("a selection");
        assert!(matches!(selection.form, SelectorForm::Regex(ref p) if p == "q[34]"));
        let names: Vec<&str> = selection
            .columns
            .iter()
            .map(|c| c.path[0].as_str())
            .collect();
        assert_eq!(names, ["q3", "q4"]);
        // The tree holds the placeholder, not the selection.
        assert_eq!(render(&ir.root), "isnull!(selected:any):bool");
    }

    #[test]
    fn columns_star_resolves_to_every_column() {
        let ir = ir("COLUMNS(*) IS NOT NULL");
        let selection = ir.selection.expect("a selection");
        assert!(matches!(selection.form, SelectorForm::All));
        assert_eq!(selection.columns.len(), TestEnv::COLUMNS.len());
    }

    #[test]
    fn a_field_reference_resolves_to_the_fields_type() {
        assert_eq!(
            rendered("LENGTH(addr.zip) > 0"),
            "Gt(Length(col(addr.zip):string):number, 0:number):bool"
        );
    }

    #[test]
    fn null_is_typeless() {
        assert_eq!(rendered("qty = NULL"), "Eq(col(qty):number, null:any):bool");
    }

    #[test]
    fn interval_units_are_resolved() {
        for (unit, expected) in [
            ("seconds", IntervalUnit::Seconds),
            ("minutes", IntervalUnit::Minutes),
            ("hours", IntervalUnit::Hours),
            ("days", IntervalUnit::Days),
            ("weeks", IntervalUnit::Weeks),
        ] {
            let ir = ir(&format!("ts >= NOW() - interval(1, {unit})"));
            let rendered = render(&ir.root);
            assert!(rendered.contains(&format!("{expected:?}")), "{unit}");
        }
        assert_eq!(IntervalUnit::Weeks.seconds(), 7 * 24 * 3600);
    }
}
