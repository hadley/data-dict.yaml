//! A faithful R expression tree.
//!
//! Deliberately R-shaped rather than a step towards the language's own AST: it
//! keeps named arguments, `~` formulas, `\(x)` lambdas, `$` and `[`, none of
//! which the language has. Those are what the [folds](super::fold) match on, and
//! keeping them here means none of them leaks into the shared AST.
//!
//! Equality **ignores spans**, because that is what a fold needs: recognising
//! `is.na(x) | (x %in% c(1, 2))` as an `IN` requires knowing the two `x` are the
//! same expression, and they are never at the same offset.

/// One node, and where in the R source it came from.
#[derive(Debug, Clone)]
pub struct RExpr {
    pub kind: RKind,
    pub span: (usize, usize),
}

impl RExpr {
    pub fn new(kind: RKind, span: (usize, usize)) -> RExpr {
        RExpr { kind, span }
    }

    /// The span covering both `self` and `other`, for a node folded out of
    /// several.
    pub fn union(&self, other: &RExpr) -> (usize, usize) {
        (self.span.0.min(other.span.0), self.span.1.max(other.span.1))
    }
}

/// Two nodes are equal when they say the same thing, wherever they were written.
impl PartialEq for RExpr {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RKind {
    /// `integer` records an `L` suffix, or a literal with no `.` or exponent —
    /// what decides whether the language reads it as an integer.
    Num {
        value: f64,
        integer: bool,
    },
    Str(String),
    Logical(bool),
    /// `NA`, and the typed `NA_integer_` family, which all mean the same here.
    Na,
    Inf,
    NaN,
    Null,
    /// An R name. R allows `.` inside one, so `is.na` is a single name, not a
    /// field access — that is `$`.
    Name(String),
    /// `x$field`.
    Dollar(Box<RExpr>, String),
    /// `x[i]`. Accepted only inside the one idiom that uses it; see
    /// [`fold`](super::fold).
    Index(Box<RExpr>, Box<RExpr>),
    Call {
        fun: String,
        args: Vec<RArg>,
    },
    Unary(RUnop, Box<RExpr>),
    Binary(RBinop, Box<RExpr>, Box<RExpr>),
    /// `\(x) body` or `function(x) body`.
    Lambda {
        param: String,
        body: Box<RExpr>,
    },
}

/// A call argument, named or not: `na.rm = TRUE` against a bare one.
#[derive(Debug, Clone, PartialEq)]
pub struct RArg {
    pub name: Option<String>,
    pub value: RExpr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RUnop {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RBinop {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    /// `%%`
    Mod,
    /// `%in%`
    In,
    /// `~`, which only ever appears inside `case_when`.
    Formula,
}

impl RBinop {
    /// R's precedence, tightest first. The inverse of `emit::r`'s own table —
    /// the two must agree or emitted R will not read back as itself. The one
    /// that surprises: an infix `%…%` binds *tighter* than `*` and `/`.
    pub fn prec(self) -> u8 {
        match self {
            RBinop::Mod | RBinop::In => 7,
            RBinop::Mul | RBinop::Div => 6,
            RBinop::Add | RBinop::Sub => 5,
            RBinop::Eq | RBinop::Ne | RBinop::Lt | RBinop::Le | RBinop::Gt | RBinop::Ge => 4,
            // `!` sits at 3, between comparison and `&`.
            RBinop::And => 2,
            RBinop::Or => 1,
            RBinop::Formula => 0,
        }
    }
}

/// The precedence of `!`, which binds looser than every comparison and tighter
/// than `&`.
pub const PREC_NOT: u8 = 3;

/// The precedence of unary `-`, which binds tighter than `%…%`.
pub const PREC_NEG: u8 = 8;

/// Helpers the folds read with, so a pattern match stays legible.
impl RExpr {
    pub fn as_call(&self, fun: &str) -> Option<&[RArg]> {
        match &self.kind {
            RKind::Call { fun: f, args } if f == fun => Some(args),
            _ => None,
        }
    }

    /// A call's arguments when every one is positional and there are `n` of
    /// them — the shape most folds want.
    pub fn as_plain_call(&self, fun: &str, n: usize) -> Option<Vec<&RExpr>> {
        let args = self.as_call(fun)?;
        (args.len() == n && args.iter().all(|a| a.name.is_none()))
            .then(|| args.iter().map(|a| &a.value).collect())
    }

    pub fn as_binary(&self, op: RBinop) -> Option<(&RExpr, &RExpr)> {
        match &self.kind {
            RKind::Binary(o, l, r) if *o == op => Some((l, r)),
            _ => None,
        }
    }

    pub fn as_unary(&self, op: RUnop) -> Option<&RExpr> {
        match &self.kind {
            RKind::Unary(o, x) if *o == op => Some(x),
            _ => None,
        }
    }

    pub fn is_na(&self) -> bool {
        matches!(self.kind, RKind::Na)
    }

    /// `is.na(X)`, giving `X`.
    pub fn as_is_na(&self) -> Option<&RExpr> {
        self.as_plain_call("is.na", 1).map(|args| args[0])
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.kind {
            RKind::Str(s) => Some(s),
            _ => None,
        }
    }
}
