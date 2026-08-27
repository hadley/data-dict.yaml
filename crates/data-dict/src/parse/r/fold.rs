//! Recognising the R idioms that mean exactly one of the language's constructs.
//!
//! The [`R` targets](crate::emit) do not emit plain R — they emit *guarded* R.
//! `COUNT(x)` comes out as `sum(!is.na(x))`, `x IN (1, 2)` as
//! `is.na(x) | (x %in% c(1, 2))`, because R's own spelling of each answers
//! differently on a missing value. Read a node at a time, those guards are
//! nonsense: `sum(!is.na(x))` is a sum over booleans, which doesn't type-check.
//!
//! So the guards are recognised and folded back. Each recogniser is **exact** —
//! every structural condition has to hold, including that the repeated operands
//! really are the same expression — and falls through rather than approximating
//! when one doesn't. A fold that fired loosely would put a rule in the
//! dictionary that nobody wrote.
//!
//! What is recognised here is what an emitter produces. An author writing the
//! bare spelling instead (`x %in% c(1, 2)` with no guard) is not folded: that
//! genuinely means something slightly different, and [`map`](super::map) reads
//! it with a note saying so.

use super::ast::{RBinop, RExpr, RKind, RUnop};

/// An R idiom that stands for exactly one of the language's constructs.
#[derive(Debug)]
pub enum Folded<'a> {
    /// `ifelse(is.na(x) | …, NA, body)` — a null guard around `body`, which the
    /// language propagates on its own.
    Unguarded(&'a RExpr),
    /// `is.na(x) | (x %in% c(…))`, the guarded membership test.
    ExactIn {
        needle: &'a RExpr,
        list: &'a RExpr,
        negated: bool,
    },
    /// `sum(!is.na(x))`.
    Count(&'a RExpr),
    /// `length(unique(x[!is.na(x)]))`, base R's distinct count.
    CountDistinct(&'a RExpr),
    /// `x >= lo & x <= hi`, base R's `BETWEEN`.
    Between {
        operand: &'a RExpr,
        lo: &'a RExpr,
        hi: &'a RExpr,
    },
}

/// The idiom `e` is, if it is one.
pub fn recognise(e: &RExpr) -> Option<Folded<'_>> {
    kind_test(e)
        .map(Folded::Unguarded)
        .or_else(|| null_guard(e).map(Folded::Unguarded))
        .or_else(|| exact_in(e))
        .or_else(|| count(e))
        .or_else(|| count_distinct(e))
        .or_else(|| between(e))
}

/// The three conditionals are one construct under three names.
fn as_conditional(e: &RExpr) -> Option<Vec<&RExpr>> {
    ["ifelse", "if_else", "fifelse"]
        .iter()
        .find_map(|name| e.as_plain_call(name, 3))
}

/// `ifelse(is.na(a) | is.na(b) | …, NA, body)` — the guard an emitter adds so a
/// function that answers `FALSE` for an `NA` propagates the null instead. The
/// language propagates on its own, so the guard is dropped.
///
/// Every guarded operand must actually appear in `body`; otherwise this is an
/// ordinary conditional that happens to test for missingness.
fn null_guard(e: &RExpr) -> Option<&RExpr> {
    let args = as_conditional(e)?;
    let [cond, na, body] = args[..] else {
        return None;
    };
    if !na.is_na() {
        return None;
    }
    let mut guarded = Vec::new();
    collect_or(cond, &mut guarded);
    let operands: Vec<&RExpr> = guarded.iter().filter_map(|g| g.as_is_na()).collect();
    if operands.len() != guarded.len() || operands.is_empty() {
        return None;
    }
    operands
        .iter()
        .all(|operand| contains(body, operand))
        .then_some(body)
}

/// `ifelse(is.na(x) & !is.nan(x), NA, is.nan(x))` and its `is.finite` /
/// `is.infinite` siblings — the guard that keeps a null null, where R's own
/// predicate answers `FALSE` for one. `is.na(x) & !is.nan(x)` is exactly "missing
/// but not a NaN", which is where the two disagree.
fn kind_test(e: &RExpr) -> Option<&RExpr> {
    let args = as_conditional(e)?;
    let [cond, na, body] = args[..] else {
        return None;
    };
    if !na.is_na() {
        return None;
    }
    let (left, right) = cond.as_binary(RBinop::And)?;
    let missing = left.as_is_na()?;
    let not_nan = right.as_unary(RUnop::Not)?.as_plain_call("is.nan", 1)?[0];
    if missing != not_nan {
        return None;
    }
    let tested = ["is.nan", "is.finite", "is.infinite"]
        .iter()
        .find_map(|name| body.as_plain_call(name, 1))?[0];
    (tested == missing).then_some(body)
}

/// `is.na(x) | (x %in% c(…))`, and the negated `is.na(x) | !(x %in% c(…))`.
/// R's `%in%` answers `FALSE` for an `NA` subject where the language's `IN`
/// gives null, so the emitter restores the null with an explicit test; reading
/// it back gives the plain `IN` again.
fn exact_in(e: &RExpr) -> Option<Folded<'_>> {
    let (left, right) = e.as_binary(RBinop::Or)?;
    let guarded = left.as_is_na()?;
    let (membership, negated) = match right.as_unary(RUnop::Not) {
        Some(inner) => (inner, true),
        None => (right, false),
    };
    let (needle, list) = membership.as_binary(RBinop::In)?;
    (needle == guarded).then_some(Folded::ExactIn {
        needle,
        list,
        negated,
    })
}

/// `sum(!is.na(x))` — the language's `COUNT`, which counts the values that are
/// present. Read literally this is a sum over booleans, which doesn't type-check,
/// so it has to be recognised before the generic `sum` rule.
fn count(e: &RExpr) -> Option<Folded<'_>> {
    let args = e.as_call("sum")?;
    // `na.rm` here would be about the booleans, not the column, so any named
    // argument means this isn't the idiom.
    let [arg] = args else { return None };
    if arg.name.is_some() {
        return None;
    }
    let operand = arg.value.as_unary(RUnop::Not)?.as_is_na()?;
    Some(Folded::Count(operand))
}

/// `length(unique(x[!is.na(x)]))` — base R's distinct count, the one idiom that
/// uses `[`.
fn count_distinct(e: &RExpr) -> Option<Folded<'_>> {
    let inner = e.as_plain_call("length", 1)?[0];
    let unique = inner.as_plain_call("unique", 1)?[0];
    let RKind::Index(subject, index) = &unique.kind else {
        return None;
    };
    let kept = index.as_unary(RUnop::Not)?.as_is_na()?;
    (kept == subject.as_ref()).then_some(Folded::CountDistinct(subject))
}

/// `x >= lo & x <= hi` — how base R spells `BETWEEN`, since it has no
/// `between()`. Folding a hand-written one is harmless: it means the same thing.
fn between(e: &RExpr) -> Option<Folded<'_>> {
    let (left, right) = e.as_binary(RBinop::And)?;
    let (lo_subject, lo) = left.as_binary(RBinop::Ge)?;
    let (hi_subject, hi) = right.as_binary(RBinop::Le)?;
    (lo_subject == hi_subject).then_some(Folded::Between {
        operand: lo_subject,
        lo,
        hi,
    })
}

/// Flatten a left-nested chain of `|` into its operands.
fn collect_or<'a>(e: &'a RExpr, out: &mut Vec<&'a RExpr>) {
    match e.as_binary(RBinop::Or) {
        Some((left, right)) => {
            collect_or(left, out);
            collect_or(right, out);
        }
        None => out.push(e),
    }
}

/// Whether `needle` appears anywhere in `haystack`.
fn contains(haystack: &RExpr, needle: &RExpr) -> bool {
    if haystack == needle {
        return true;
    }
    match &haystack.kind {
        RKind::Unary(_, x) => contains(x, needle),
        RKind::Binary(_, l, r) => contains(l, needle) || contains(r, needle),
        RKind::Dollar(x, _) => contains(x, needle),
        RKind::Index(x, i) => contains(x, needle) || contains(i, needle),
        RKind::Lambda { body, .. } => contains(body, needle),
        RKind::Call { args, .. } => args.iter().any(|a| contains(&a.value, needle)),
        _ => false,
    }
}
