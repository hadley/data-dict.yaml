//! Evaluating a checked assertion against data: the reference implementation of
//! [`site/expression-execution.md`](https://data-dict.tidyverse.org/expression-execution.html).
//!
//! A row **passes** when the expression is `true` or `null`, and only `false` is
//! a violation. What a violation can name follows the assertion's
//! [`Shape`](crate::assert_expr::Shape): a `row` assertion reports the rows that
//! broke it, an `agg` one only that it is false for the table.
//!
//! Two things stop an assertion reaching a verdict at all, and each withdraws it
//! rather than guess: integer arithmetic leaving the 64-bit range, and a pattern
//! read from the data that isn't a valid regex. Everything else about the data
//! yields a value, a zero divisor included — `7 / 0` is an infinity and `0 / 0` a
//! NaN, as [`site/floating-point.md`](https://data-dict.tidyverse.org/floating-point.html)
//! describes.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, Utc};
use data_dict_parquet::{
    Batch, ParquetError, TypedColumnRequest, TypedValues, ValueType, read_typed,
};

use crate::assert_expr::{
    ArithOp, CmpOp, ColumnRef, DatetimeConst, LikePattern, NodeKind, Op, Shape, Type,
    TypedAssertion, TypedExpr,
};

/// Days from the epoch to a date, the form dates are compared in.
const MICROS_PER_DAY: i64 = 86_400_000_000;

/// A value during evaluation. Temporal values are integers in a fixed unit so
/// that a `date` and a `datetime` compare without a conversion at every step.
#[derive(Debug, Clone, PartialEq)]
enum Value<'a> {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Cow<'a, str>),
    /// Days since 1970-01-01.
    Date(i64),
    /// Microseconds since 1970-01-01T00:00:00.
    Datetime(i64),
    /// A duration in seconds; every unit the language has is fixed-length.
    Interval(i64),
}

impl Value<'_> {
    fn into_owned(self) -> Value<'static> {
        match self {
            Value::Null => Value::Null,
            Value::Int(n) => Value::Int(n),
            Value::Float(x) => Value::Float(x),
            Value::Bool(b) => Value::Bool(b),
            Value::Str(s) => Value::Str(Cow::Owned(s.into_owned())),
            Value::Date(d) => Value::Date(d),
            Value::Datetime(t) => Value::Datetime(t),
            Value::Interval(i) => Value::Interval(i),
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(n) => Some(*n as f64),
            Value::Float(x) => Some(*x),
            _ => None,
        }
    }

    /// Microseconds, for comparing a date against a datetime: a date is read as
    /// midnight, as [shifting one](https://data-dict.tidyverse.org/expressions.html#arithmetic) also assumes.
    fn as_micros(&self) -> Option<i64> {
        match self {
            Value::Date(days) => days.checked_mul(MICROS_PER_DAY),
            Value::Datetime(t) => Some(*t),
            _ => None,
        }
    }

    /// What `COUNT_DISTINCT` folds on: the rendering, with a float canonicalized
    /// first so that the values the [identity order](identity_compare) counts as
    /// one — both zeros, every NaN — share a key.
    fn distinct_key(&self) -> String {
        match self {
            Value::Float(x) => Value::Float(canonical(*x)).render(),
            v => v.render(),
        }
    }

    fn render(&self) -> String {
        match self {
            Value::Null => "null".to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(x) => x.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => s.to_string(),
            Value::Date(days) => epoch()
                .checked_add_signed(chrono::Duration::days(*days))
                .map_or_else(|| days.to_string(), |d| d.to_string()),
            Value::Datetime(micros) => DateTime::from_timestamp_micros(*micros)
                .map_or_else(|| micros.to_string(), |t| t.naive_utc().to_string()),
            Value::Interval(seconds) => format!("{seconds}s"),
        }
    }
}

/// Why an assertion could not reach a verdict. Each replaces the verdict rather
/// than standing in for one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fault {
    Overflow,
    /// A `LIKE` or `SIMILAR TO` pattern computed from the data is not a valid
    /// regex. A literal pattern can't get here — S21 rejects it at the spec
    /// level — so the offending text is worth carrying.
    BadPattern(String),
}

type Eval<'a> = Result<Value<'a>, Fault>;

/// What evaluating one assertion over one table found.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// A row-shaped assertion: the rows it is false for.
    Rows {
        count: usize,
        rows: Vec<usize>,
        samples: Vec<String>,
    },
    /// An aggregate or constant assertion: one verdict for the table.
    Table { holds: bool },
    /// The arithmetic had no result, so there is no verdict.
    Faulted { fault: Fault, row: Option<usize> },
}

/// Evaluate `assertion` against the parquet file at `path`.
///
/// `now` is passed in rather than read here so that every assertion in one
/// `validate-data` run shares a single reading of the clock.
pub(crate) fn evaluate(
    path: &std::path::Path,
    assertion: &TypedAssertion,
    now: DateTime<Utc>,
    sample_limit: usize,
) -> Result<Outcome, ParquetError> {
    let plan = Plan::build(assertion);
    let requests = plan.requests();

    // An aggregate folds the whole table, so its value has to be known before
    // any row can be judged. That costs one extra pass over the same columns.
    let aggregates = if plan.has_aggregates() {
        Some(plan.fold_aggregates(path, &requests, now)?)
    } else {
        None
    };

    plan.judge(path, &requests, now, aggregates.as_deref(), sample_limit)
}

/// Regexes compiled once for the whole evaluation instead of once per row.
/// Keyed by the pattern's source text, with one cache per spelling: the same
/// text means different things as a `LIKE` pattern and as a regex. `None` marks
/// a source that does not compile, so a bad pattern is diagnosed once too.
#[derive(Default)]
struct Patterns {
    /// A regex the IR built from a literal `LIKE` pattern, already anchored.
    verbatim: RefCell<HashMap<String, Option<regex::Regex>>>,
    /// A `LIKE` pattern computed from the data, still in `%`/`_` form.
    like: RefCell<HashMap<String, Option<regex::Regex>>>,
    /// A `SIMILAR TO` pattern, anchored when compiled.
    similar: RefCell<HashMap<String, Option<regex::Regex>>>,
}

impl Patterns {
    fn matches(
        cache: &RefCell<HashMap<String, Option<regex::Regex>>>,
        source: &str,
        subject: &str,
        as_regex: impl FnOnce(&str) -> String,
    ) -> Result<bool, Fault> {
        let mut cache = cache.borrow_mut();
        if !cache.contains_key(source) {
            let compiled = regex::Regex::new(&as_regex(source)).ok();
            cache.insert(source.to_string(), compiled);
        }
        match &cache[source] {
            Some(re) => Ok(re.is_match(subject)),
            None => Err(Fault::BadPattern(source.to_string())),
        }
    }
}

/// The columns an assertion reads, and the instantiations to evaluate.
struct Plan<'a> {
    assertion: &'a TypedAssertion,
    /// Every column the expression names, in request order.
    columns: Vec<ColumnRef>,
    /// One entry per `COLUMNS(...)` column, giving the index in `columns` that
    /// `Selected` stands for. Exactly one `None` entry when there is no
    /// selection, so an expression without one is just the single-instance case.
    instances: Vec<Option<usize>>,
    /// The topmost aggregate subtrees, by span, in evaluation order.
    agg_spans: Vec<(usize, usize)>,
    patterns: Patterns,
}

impl<'a> Plan<'a> {
    fn build(assertion: &'a TypedAssertion) -> Plan<'a> {
        let mut columns: Vec<ColumnRef> = Vec::new();
        collect_columns(&assertion.root, &mut columns);

        let instances = match &assertion.selection {
            None => vec![None],
            Some(selection) => selection
                .columns
                .iter()
                .map(|c| {
                    let index = index_of(&mut columns, c);
                    Some(index)
                })
                .collect(),
        };

        let mut agg_spans = Vec::new();
        collect_aggregates(&assertion.root, &mut agg_spans);

        Plan {
            assertion,
            columns,
            instances,
            agg_spans,
            patterns: Patterns::default(),
        }
    }

    fn requests(&self) -> Vec<TypedColumnRequest> {
        self.columns
            .iter()
            .map(|c| TypedColumnRequest {
                path: c.path.clone(),
                ty: value_type(c.ty),
            })
            .collect()
    }

    fn has_aggregates(&self) -> bool {
        !self.agg_spans.is_empty()
    }

    /// Pass one: fold every aggregate subtree over the whole table, for every
    /// instantiation, since a selection may put a different column under each.
    fn fold_aggregates(
        &self,
        path: &std::path::Path,
        requests: &[TypedColumnRequest],
        now: DateTime<Utc>,
    ) -> Result<Vec<Vec<Value<'static>>>, ParquetError> {
        let mut slots: Vec<Vec<Accumulator>> = self
            .instances
            .iter()
            .map(|_| self.agg_spans.iter().map(|_| Accumulator::new()).collect())
            .collect();

        for batch in read_typed(path, requests)? {
            let batch = batch?;
            for row in 0..batch.rows() {
                for (instance, selected) in self.instances.iter().enumerate() {
                    let cx = Cx {
                        batch: Some(&batch),
                        row,
                        columns: &self.columns,
                        selected: *selected,
                        now,
                        agg_spans: &self.agg_spans,
                        aggregates: None,
                        patterns: &self.patterns,
                    };
                    for (slot, span) in self.agg_spans.iter().enumerate() {
                        let node = find_span(&self.assertion.root, *span)
                            .expect("aggregate span was collected from this tree");
                        slots[instance][slot].observe(&cx, node);
                    }
                }
            }
        }

        Ok(slots
            .into_iter()
            .map(|row| row.into_iter().map(Accumulator::finish).collect())
            .collect())
    }

    /// Pass two: judge each row, or the table when the assertion is aggregate.
    fn judge(
        &self,
        path: &std::path::Path,
        requests: &[TypedColumnRequest],
        now: DateTime<Utc>,
        aggregates: Option<&[Vec<Value<'static>>]>,
        sample_limit: usize,
    ) -> Result<Outcome, ParquetError> {
        // An assertion with no row-level part is one verdict about the table,
        // and needs no per-row pass to reach it.
        if self.assertion.root.shape != Shape::Row {
            return Ok(self.judge_table(now, aggregates));
        }

        let mut count = 0usize;
        let mut rows = Vec::new();
        let mut samples = Vec::new();

        for batch in read_typed(path, requests)? {
            let batch = batch?;
            for row in 0..batch.rows() {
                let absolute = batch.first_row() + row + 1;
                for (instance, selected) in self.instances.iter().enumerate() {
                    let cx = Cx {
                        batch: Some(&batch),
                        row,
                        columns: &self.columns,
                        selected: *selected,
                        now,
                        agg_spans: &self.agg_spans,
                        aggregates: aggregates.map(|a| a[instance].as_slice()),
                        patterns: &self.patterns,
                    };
                    match eval(&cx, &self.assertion.root) {
                        Err(fault) => {
                            return Ok(Outcome::Faulted {
                                fault,
                                row: Some(absolute),
                            });
                        }
                        // Only `false` is a violation; `true` and null pass.
                        Ok(Value::Bool(false)) => {
                            count += 1;
                            if rows.len() < sample_limit {
                                rows.push(absolute);
                                samples.push(self.sample(&cx));
                            }
                            // One row breaks the assertion once, however many
                            // selected columns break it.
                            break;
                        }
                        Ok(_) => {}
                    }
                }
            }
        }

        Ok(Outcome::Rows {
            count,
            rows,
            samples,
        })
    }

    fn judge_table(
        &self,
        now: DateTime<Utc>,
        aggregates: Option<&[Vec<Value<'static>>]>,
    ) -> Outcome {
        // With no rows to read from, only aggregate and constant subexpressions
        // can appear, and those need no batch.
        for (instance, selected) in self.instances.iter().enumerate() {
            let cx = Cx {
                batch: None,
                row: 0,
                columns: &self.columns,
                selected: *selected,
                now,
                agg_spans: &self.agg_spans,
                aggregates: aggregates.map(|a| a[instance].as_slice()),
                patterns: &self.patterns,
            };
            match eval(&cx, &self.assertion.root) {
                Err(fault) => return Outcome::Faulted { fault, row: None },
                Ok(Value::Bool(false)) => return Outcome::Table { holds: false },
                Ok(_) => {}
            }
        }
        Outcome::Table { holds: true }
    }

    /// The values of the columns the expression reads, for a violating row.
    fn sample(&self, cx: &Cx) -> String {
        let mut parts = Vec::new();
        for (i, column) in self.columns.iter().enumerate() {
            let value = cx.column(i).map_or(Value::Null, |v| v);
            parts.push(format!("{}={}", column.path.join("."), value.render()));
        }
        parts.join(", ")
    }
}

fn value_type(ty: Type) -> ValueType {
    match ty {
        Type::Number => ValueType::Number,
        Type::Bool => ValueType::Bool,
        Type::Date => ValueType::Date,
        Type::Datetime => ValueType::Datetime,
        // A `COLUMNS(...)` column of unknown type, or a string: either way the
        // only reading that can work is as text.
        Type::String | Type::Interval | Type::Any => ValueType::String,
    }
}

fn index_of(columns: &mut Vec<ColumnRef>, wanted: &ColumnRef) -> usize {
    if let Some(i) = columns.iter().position(|c| c.path == wanted.path) {
        return i;
    }
    columns.push(wanted.clone());
    columns.len() - 1
}

fn collect_columns(e: &TypedExpr, out: &mut Vec<ColumnRef>) {
    if let NodeKind::Column(c) = &e.kind {
        index_of(out, c);
    }
    for child in e.children() {
        collect_columns(child, out);
    }
}

/// The topmost aggregate subtrees. An aggregate can't contain another (S30), so
/// there is no need to descend into one.
fn collect_aggregates(e: &TypedExpr, out: &mut Vec<(usize, usize)>) {
    if e.shape == Shape::Agg
        && let NodeKind::Func { op, .. } = &e.kind
        && op.is_aggregate()
    {
        out.push(e.span);
        return;
    }
    for child in e.children() {
        collect_aggregates(child, out);
    }
}

fn find_span(e: &TypedExpr, span: (usize, usize)) -> Option<&TypedExpr> {
    if e.span == span {
        return Some(e);
    }
    e.children().into_iter().find_map(|c| find_span(c, span))
}

/// Everything one row's evaluation needs.
struct Cx<'a> {
    /// `None` when judging a table-shaped assertion, which reads no rows.
    batch: Option<&'a Batch>,
    row: usize,
    /// The columns read, in request order, so a reference can find its own.
    columns: &'a [ColumnRef],
    /// Which column `Selected` stands for in this instantiation.
    selected: Option<usize>,
    now: DateTime<Utc>,
    /// The aggregate subtrees, by span, paired with their folded values.
    agg_spans: &'a [(usize, usize)],
    aggregates: Option<&'a [Value<'static>]>,
    patterns: &'a Patterns,
}

impl<'a> Cx<'a> {
    /// The value of the `i`th requested column in this row, or `None` for null.
    fn column(&self, i: usize) -> Option<Value<'a>> {
        let column = self.batch?.column(i);
        if !column.is_valid(self.row) {
            return None;
        }
        Some(match column.values() {
            TypedValues::Int(v) => Value::Int(v[self.row]),
            TypedValues::Float(v) => Value::Float(v[self.row]),
            TypedValues::Bool(v) => Value::Bool(v.get(self.row)),
            TypedValues::Str(v) => Value::Str(Cow::Borrowed(v.get(self.row))),
            TypedValues::Date(v) => Value::Date(v[self.row] as i64),
            TypedValues::Datetime { micros, .. } => Value::Datetime(micros[self.row]),
        })
    }

    fn index_of(&self, c: &ColumnRef) -> Option<usize> {
        self.columns.iter().position(|k| k.path == c.path)
    }
}

fn eval<'a>(cx: &Cx<'a>, e: &'a TypedExpr) -> Eval<'a> {
    // An aggregate was folded before any row was judged; substitute its value.
    if let Some(values) = cx.aggregates
        && let Some(slot) = cx.agg_spans.iter().position(|s| *s == e.span)
    {
        return Ok(values[slot].clone());
    }

    Ok(match &e.kind {
        NodeKind::Int(n) => Value::Int(*n),
        NodeKind::Float(x) => Value::Float(*x),
        NodeKind::Str(s) => Value::Str(Cow::Borrowed(s)),
        NodeKind::Bool(b) => Value::Bool(*b),
        NodeKind::Null => Value::Null,
        NodeKind::Date(d) => Value::Date(days_from_epoch(*d)),
        NodeKind::Datetime(t) => Value::Datetime(match t {
            DatetimeConst::Offset(t) => t.timestamp_micros(),
            DatetimeConst::Naive(t) => t.and_utc().timestamp_micros(),
        }),
        NodeKind::Now => Value::Datetime(cx.now.timestamp_micros()),
        NodeKind::Column(c) => cx
            .index_of(c)
            .and_then(|i| cx.column(i))
            .unwrap_or(Value::Null),
        NodeKind::Selected => match cx.selected {
            Some(i) => cx.column(i).unwrap_or(Value::Null),
            None => Value::Null,
        },
        NodeKind::Neg(x) => match eval(cx, x)? {
            Value::Null => Value::Null,
            Value::Int(n) => Value::Int(n.checked_neg().ok_or(Fault::Overflow)?),
            Value::Float(x) => Value::Float(-x),
            _ => Value::Null,
        },
        NodeKind::Not(x) => match eval(cx, x)?.as_bool() {
            Some(b) => Value::Bool(!b),
            None => Value::Null,
        },
        NodeKind::And(l, r) => {
            // Short-circuit on a decisive operand, as three-valued logic does:
            // `false AND null` is false.
            let l = eval(cx, l)?;
            if l.as_bool() == Some(false) {
                return Ok(Value::Bool(false));
            }
            let r = eval(cx, r)?;
            if r.as_bool() == Some(false) {
                return Ok(Value::Bool(false));
            }
            match (l.as_bool(), r.as_bool()) {
                (Some(true), Some(true)) => Value::Bool(true),
                _ => Value::Null,
            }
        }
        NodeKind::Or(l, r) => {
            let l = eval(cx, l)?;
            if l.as_bool() == Some(true) {
                return Ok(Value::Bool(true));
            }
            let r = eval(cx, r)?;
            if r.as_bool() == Some(true) {
                return Ok(Value::Bool(true));
            }
            match (l.as_bool(), r.as_bool()) {
                (Some(false), Some(false)) => Value::Bool(false),
                _ => Value::Null,
            }
        }
        NodeKind::Arith { op, lhs, rhs } => arith(*op, eval(cx, lhs)?, eval(cx, rhs)?)?,
        NodeKind::Compare { op, lhs, rhs } => match compare(&eval(cx, lhs)?, &eval(cx, rhs)?) {
            Cmp::Unknown => Value::Null,
            // Nothing equals, precedes or follows a NaN, so only `<>` holds.
            Cmp::Unordered => Value::Bool(*op == CmpOp::Ne),
            Cmp::Ordered(ordering) => Value::Bool(match op {
                CmpOp::Eq => ordering == 0,
                CmpOp::Ne => ordering != 0,
                CmpOp::Lt => ordering < 0,
                CmpOp::Le => ordering <= 0,
                CmpOp::Gt => ordering > 0,
                CmpOp::Ge => ordering >= 0,
            }),
        },
        NodeKind::IsNull { operand, negated } => {
            let is_null = matches!(eval(cx, operand)?, Value::Null);
            Value::Bool(is_null != *negated)
        }
        NodeKind::Between {
            operand,
            lo,
            hi,
            negated,
        } => {
            let v = eval(cx, operand)?;
            let (lo, hi) = (eval(cx, lo)?, eval(cx, hi)?);
            match (compare(&v, &lo), compare(&v, &hi)) {
                (Cmp::Unknown, _) | (_, Cmp::Unknown) => Value::Null,
                // A NaN lies within nothing, as the comparisons it stands for say.
                (Cmp::Unordered, _) | (_, Cmp::Unordered) => Value::Bool(*negated),
                (Cmp::Ordered(a), Cmp::Ordered(b)) => {
                    Value::Bool(((a >= 0) && (b <= 0)) != *negated)
                }
            }
        }
        NodeKind::In {
            needle,
            haystack,
            negated,
        } => {
            let v = eval(cx, needle)?;
            if matches!(v, Value::Null) {
                return Ok(Value::Null);
            }
            let mut found = false;
            let mut unknown = false;
            for item in haystack {
                match compare(&v, &eval(cx, item)?) {
                    Cmp::Ordered(0) => found = true,
                    // A NaN matches nothing, which is an answer, not an unknown.
                    Cmp::Ordered(_) | Cmp::Unordered => {}
                    Cmp::Unknown => unknown = true,
                }
            }
            // No match plus an unknown means the answer is unknown, as SQL's
            // `IN` does with a null in the list.
            match (found, unknown) {
                (true, _) => Value::Bool(!*negated),
                (false, true) => Value::Null,
                (false, false) => Value::Bool(*negated),
            }
        }
        NodeKind::Like {
            operand,
            pattern,
            negated,
        } => {
            let Value::Str(s) = eval(cx, operand)? else {
                return Ok(Value::Null);
            };
            let matched = match pattern {
                LikePattern::Exact(p) => s == p.as_str(),
                LikePattern::Prefix(p) => s.starts_with(p.as_str()),
                LikePattern::Suffix(p) => s.ends_with(p.as_str()),
                LikePattern::Regex(p) => {
                    Patterns::matches(&cx.patterns.verbatim, p, &s, str::to_string)?
                }
                LikePattern::Dynamic(p) => {
                    let Value::Str(p) = eval(cx, p)? else {
                        return Ok(Value::Null);
                    };
                    Patterns::matches(&cx.patterns.like, &p, &s, like_to_regex)?
                }
            };
            Value::Bool(matched != *negated)
        }
        NodeKind::SimilarTo {
            operand,
            pattern,
            negated,
        } => {
            let (Value::Str(s), Value::Str(p)) = (eval(cx, operand)?, eval(cx, pattern)?) else {
                return Ok(Value::Null);
            };
            // Anchored, so the pattern must match the whole string.
            let matched =
                Patterns::matches(&cx.patterns.similar, &p, &s, |p| format!("^(?:{p})$"))?;
            Value::Bool(matched != *negated)
        }
        NodeKind::Interval { n, unit } => match eval(cx, n)? {
            Value::Null => Value::Null,
            v => {
                let n = v.as_f64().unwrap_or(0.0) as i64;
                Value::Interval(n.checked_mul(unit.seconds()).ok_or(Fault::Overflow)?)
            }
        },
        NodeKind::Case { whens, els } => {
            for (cond, result) in whens {
                if eval(cx, cond)?.as_bool() == Some(true) {
                    return eval(cx, result);
                }
            }
            match els {
                Some(e) => eval(cx, e)?,
                None => Value::Null,
            }
        }
        NodeKind::Func { op, args } => func(cx, *op, args)?,
    })
}

fn days_from_epoch(d: NaiveDate) -> i64 {
    d.signed_duration_since(epoch()).num_days()
}

fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).expect("1970-01-01 is a date")
}

fn arith<'a>(op: ArithOp, l: Value<'a>, r: Value<'a>) -> Eval<'a> {
    if matches!(l, Value::Null) || matches!(r, Value::Null) {
        return Ok(Value::Null);
    }
    // A date or datetime shifted by an interval is a datetime.
    if let Some(shifted) = shift(op, &l, &r)? {
        return Ok(shifted);
    }
    match (&l, &r) {
        // Integer arithmetic stays exact, and overflowing it is a fault rather
        // than a wrapped answer.
        (Value::Int(a), Value::Int(b)) if op != ArithOp::Div => {
            let out = match op {
                ArithOp::Add => a.checked_add(*b),
                ArithOp::Sub => a.checked_sub(*b),
                ArithOp::Mul => a.checked_mul(*b),
                ArithOp::Div => unreachable!(),
            };
            Ok(Value::Int(out.ok_or(Fault::Overflow)?))
        }
        _ => {
            let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) else {
                return Ok(Value::Null);
            };
            Ok(Value::Float(match op {
                ArithOp::Add => a + b,
                ArithOp::Sub => a - b,
                ArithOp::Mul => a * b,
                // `/` always yields a float, and a zero divisor an infinity or a
                // NaN, as IEEE 754 says.
                ArithOp::Div => a / b,
            }))
        }
    }
}

/// A temporal value plus or minus an interval, if that is what this is.
fn shift<'a>(op: ArithOp, l: &Value<'a>, r: &Value<'a>) -> Result<Option<Value<'a>>, Fault> {
    let (base, seconds) = match (l, r) {
        (Value::Interval(i), other) if op == ArithOp::Add => (other, *i),
        (other, Value::Interval(i)) => (other, if op == ArithOp::Sub { -*i } else { *i }),
        _ => return Ok(None),
    };
    let Some(micros) = base.as_micros() else {
        return Ok(None);
    };
    let delta = seconds.checked_mul(1_000_000).ok_or(Fault::Overflow)?;
    Ok(Some(Value::Datetime(
        micros.checked_add(delta).ok_or(Fault::Overflow)?,
    )))
}

/// How two values compare. A NaN is [`Cmp::Unordered`] rather than unknown: the
/// answer is known to be `false` (and `true` for `<>`), where an unknown answers
/// null.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmp {
    Ordered(i32),
    /// A NaN on either side, which IEEE 754 orders against nothing.
    Unordered,
    /// Either value is null, or the two types can't be compared at all.
    Unknown,
}

/// Order two values. A date and a datetime compare as instants, the date at
/// midnight.
fn compare(a: &Value, b: &Value) -> Cmp {
    let ordering = match (a, b) {
        (Value::Null, _) | (_, Value::Null) => return Cmp::Unknown,
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::Interval(x), Value::Interval(y)) => x.cmp(y),
        _ => {
            if let (Some(x), Some(y)) = (a.as_micros(), b.as_micros()) {
                x.cmp(&y)
            } else {
                let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) else {
                    return Cmp::Unknown;
                };
                match x.partial_cmp(&y) {
                    Some(ordering) => ordering,
                    None => return Cmp::Unordered,
                }
            }
        }
    };
    Cmp::Ordered(match ordering {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    })
}

/// The identity order: `-INF` < every finite number < `INF` < NaN, with all NaNs
/// and both zeros counting as one value each. This is what asks whether two
/// values are *the same value*, which `=` can't answer for a NaN, and it governs
/// `MIN`, `MAX` and `COUNT_DISTINCT`.
fn identity_compare(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Float(_), _) | (_, Value::Float(_)) => {
            let (x, y) = (a.as_f64()?, b.as_f64()?);
            Some(canonical(x).total_cmp(&canonical(y)))
        }
        _ => match compare(a, b) {
            Cmp::Ordered(o) => Some(o.cmp(&0)),
            Cmp::Unordered | Cmp::Unknown => None,
        },
    }
}

/// Collapse the values the identity order counts as one: both zeros, and every
/// NaN bit pattern (to a positive NaN, which `total_cmp` then sorts above `INF`).
fn canonical(x: f64) -> f64 {
    if x == 0.0 {
        0.0
    } else if x.is_nan() {
        f64::NAN
    } else {
        x
    }
}

fn func<'a>(cx: &Cx<'a>, op: Op, args: &'a [TypedExpr]) -> Eval<'a> {
    // Aggregates are folded in the first pass; reaching one here means there
    // was no such pass, which only happens for an empty table.
    if op.is_aggregate() {
        return Ok(empty_aggregate(op));
    }
    let mut values = Vec::with_capacity(args.len());
    for a in args {
        values.push(eval(cx, a)?);
    }
    if values.iter().any(|v| matches!(v, Value::Null)) {
        return Ok(Value::Null);
    }
    Ok(match (op, values.as_slice()) {
        (Op::Length, [Value::Str(s)]) => Value::Int(s.chars().count() as i64),
        (Op::Lower, [Value::Str(s)]) => Value::Str(Cow::Owned(s.to_lowercase())),
        (Op::Upper, [Value::Str(s)]) => Value::Str(Cow::Owned(s.to_uppercase())),
        (Op::Trim, [Value::Str(s)]) => Value::Str(Cow::Owned(s.trim().to_string())),
        (Op::StartsWith, [Value::Str(s), Value::Str(p)]) => Value::Bool(s.starts_with(p.as_ref())),
        (Op::EndsWith, [Value::Str(s), Value::Str(p)]) => Value::Bool(s.ends_with(p.as_ref())),
        (Op::Abs, [Value::Int(n)]) => Value::Int(n.checked_abs().ok_or(Fault::Overflow)?),
        (Op::Abs, [v]) => Value::Float(v.as_f64().unwrap_or_default().abs()),
        (Op::Floor, [v]) => Value::Float(v.as_f64().unwrap_or_default().floor()),
        (Op::Ceil, [v]) => Value::Float(v.as_f64().unwrap_or_default().ceil()),
        (Op::Round, [v]) => Value::Float(round_half_away(v.as_f64().unwrap_or_default(), 0)),
        (Op::Round, [v, d]) => Value::Float(round_half_away(
            v.as_f64().unwrap_or_default(),
            d.as_f64().unwrap_or_default() as i32,
        )),
        // A NaN is a float, so this is the one place `MOD` over two integers
        // does not give an integer.
        (Op::Mod, [Value::Int(_), Value::Int(0)]) => Value::Float(f64::NAN),
        (Op::Mod, [Value::Int(a), Value::Int(b)]) => Value::Int(floored_rem_int(*a, *b)),
        (Op::Mod, [a, b]) => {
            let (a, b) = (
                a.as_f64().unwrap_or_default(),
                b.as_f64().unwrap_or_default(),
            );
            Value::Float(floored_rem_float(a, b))
        }
        (Op::IsFinite, [v]) => Value::Bool(v.as_f64().is_some_and(f64::is_finite)),
        (Op::IsInfinite, [v]) => Value::Bool(v.as_f64().is_some_and(f64::is_infinite)),
        (Op::IsNan, [v]) => Value::Bool(v.as_f64().is_some_and(f64::is_nan)),
        _ => Value::Null,
    })
}

/// `MOD` takes its sign from the divisor, Rust's `%` from the dividend, so a
/// remainder that came out on the wrong side is moved by one divisor. `b` must
/// not be zero.
///
/// This never overflows: `wrapping_rem` only wraps for `i64::MIN % -1`, whose
/// remainder is zero either way, and the correction adds two values of opposite
/// sign.
fn floored_rem_int(a: i64, b: i64) -> i64 {
    let r = a.wrapping_rem(b);
    if r != 0 && (r < 0) != (b < 0) {
        r + b
    } else {
        r
    }
}

/// The float counterpart of [`floored_rem_int`], and what Python's `%` computes.
/// R reaches the same answer as `x - floor(x / y) * y`, which loses precision
/// where this form does not.
fn floored_rem_float(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && (r < 0.0) != (b < 0.0) {
        r + b
    } else {
        r
    }
}

/// The language rounds halves away from zero, unlike Rust's `round` for
/// negative halves — which agrees — but `digits` needs the scaling anyway.
///
/// `digits` far enough either way puts the rounding place outside what a float
/// can represent, and both ends have an exact answer: scaling to infinity means
/// the place is finer than `x`'s own precision, so `x` is already rounded, and a
/// factor that underflows to zero means the place is coarser than any float, so
/// every value rounds to zero. Neither is an overflow — no integer is involved.
fn round_half_away(x: f64, digits: i32) -> f64 {
    let factor = 10f64.powi(digits);
    let scaled = x * factor;
    if !scaled.is_finite() {
        return x;
    }
    let rounded = scaled.round();
    if factor == 0.0 {
        return rounded;
    }
    rounded / factor
}

/// What an aggregate gives back with nothing to fold, which is also its value
/// on an all-null column.
fn empty_aggregate(op: Op) -> Value<'static> {
    match op {
        Op::Count | Op::CountDistinct | Op::RowCount => Value::Int(0),
        _ => Value::Null,
    }
}

/// One aggregate's running state.
struct Accumulator {
    op: Option<Op>,
    rows: usize,
    seen: usize,
    int_sum: Option<i64>,
    float_sum: f64,
    saw_float: bool,
    extreme: Option<Value<'static>>,
    distinct: HashSet<String>,
    any: bool,
    all: bool,
    fault: Option<Fault>,
}

impl Accumulator {
    fn new() -> Accumulator {
        Accumulator {
            op: None,
            rows: 0,
            seen: 0,
            int_sum: Some(0),
            float_sum: 0.0,
            saw_float: false,
            extreme: None,
            distinct: HashSet::new(),
            any: false,
            all: true,
            fault: None,
        }
    }

    fn observe(&mut self, cx: &Cx, node: &TypedExpr) {
        let NodeKind::Func { op, args } = &node.kind else {
            return;
        };
        self.op = Some(*op);
        self.rows += 1;
        if *op == Op::RowCount {
            return;
        }
        let Some(arg) = args.first() else { return };
        let value = match eval(cx, arg) {
            Ok(v) => v,
            Err(fault) => {
                self.fault.get_or_insert(fault);
                return;
            }
        };
        // Aggregates skip nulls rather than propagating them.
        if matches!(value, Value::Null) {
            return;
        }
        self.seen += 1;
        match op {
            Op::Count => {}
            Op::CountDistinct => {
                self.distinct.insert(value.distinct_key());
            }
            Op::Sum => match &value {
                Value::Int(n) => {
                    self.int_sum = self.int_sum.and_then(|s| s.checked_add(*n));
                    if self.int_sum.is_none() {
                        self.fault.get_or_insert(Fault::Overflow);
                    }
                    self.float_sum += *n as f64;
                }
                v => {
                    self.saw_float = true;
                    self.float_sum += v.as_f64().unwrap_or_default();
                }
            },
            Op::Avg => self.float_sum += value.as_f64().unwrap_or_default(),
            Op::Any | Op::All => {
                let b = value.as_bool().unwrap_or(false);
                self.any |= b;
                self.all &= b;
            }
            Op::Min | Op::Max => {
                let replace = match &self.extreme {
                    None => true,
                    Some(current) => match identity_compare(&value, current) {
                        Some(o) => (*op == Op::Min && o.is_lt()) || (*op == Op::Max && o.is_gt()),
                        None => false,
                    },
                };
                if replace {
                    self.extreme = Some(value.into_owned());
                }
            }
            _ => {}
        }
    }

    fn finish(self) -> Value<'static> {
        let Some(op) = self.op else {
            return Value::Null;
        };
        match op {
            Op::RowCount => Value::Int(self.rows as i64),
            Op::Count => Value::Int(self.seen as i64),
            Op::CountDistinct => Value::Int(self.distinct.len() as i64),
            _ if self.seen == 0 => empty_aggregate(op),
            Op::Sum => match (self.saw_float, self.int_sum) {
                (false, Some(n)) => Value::Int(n),
                _ => Value::Float(self.float_sum),
            },
            // An average is always a float, even over integers.
            Op::Avg => Value::Float(self.float_sum / self.seen as f64),
            Op::Any => Value::Bool(self.any),
            Op::All => Value::Bool(self.all),
            Op::Min | Op::Max => self.extreme.unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }
}

/// A `LIKE` pattern computed at evaluation time, as an anchored regex.
fn like_to_regex(pattern: &str) -> String {
    let mut re = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '%' => re.push_str(".*"),
            '_' => re.push('.'),
            other => re.push_str(&regex::escape(&other.to_string())),
        }
    }
    re.push('$');
    re
}

/// The columns an assertion reads, with the type each must be read as — what a
/// caller checks against the data before deciding the assertion can run.
pub(crate) fn column_requests(assertion: &TypedAssertion) -> Vec<TypedColumnRequest> {
    Plan::build(assertion).requests()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remainder_takes_its_sign_from_the_divisor() {
        assert_eq!(floored_rem_int(7, 3), 1);
        assert_eq!(floored_rem_int(-7, 3), 2);
        assert_eq!(floored_rem_int(7, -3), -2);
        assert_eq!(floored_rem_int(-7, -3), -1);
        assert_eq!(floored_rem_int(6, 3), 0);
        assert_eq!(floored_rem_int(-6, 3), 0);
        // The one case Rust's `%` cannot compute, and the reason for the
        // wrapping remainder.
        assert_eq!(floored_rem_int(i64::MIN, -1), 0);
        assert_eq!(floored_rem_int(i64::MIN, i64::MAX), i64::MAX - 1);
        assert_eq!(floored_rem_int(i64::MAX, i64::MIN), -1);
    }

    #[test]
    fn a_float_remainder_follows_the_same_rule() {
        assert_eq!(floored_rem_float(-7.0, 2.5), 0.5);
        assert_eq!(floored_rem_float(7.0, -2.5), -0.5);
        assert_eq!(floored_rem_float(7.5, 2.5), 0.0);
    }
}
