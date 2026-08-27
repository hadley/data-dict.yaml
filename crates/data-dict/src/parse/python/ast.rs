//! A faithful Python expression tree.
//!
//! Python-shaped rather than a step towards the language's own AST: it keeps
//! attribute access, keyword arguments and list displays, none of which the
//! language has. Keeping them here means none of them leaks into the shared AST.

/// One node, and where in the Python source it came from.
#[derive(Debug, Clone)]
pub struct PyExpr {
    pub kind: PyKind,
    pub span: (usize, usize),
}

impl PyExpr {
    pub fn new(kind: PyKind, span: (usize, usize)) -> PyExpr {
        PyExpr { kind, span }
    }

    pub fn union(&self, other: &PyExpr) -> (usize, usize) {
        (self.span.0.min(other.span.0), self.span.1.max(other.span.1))
    }
}

/// Two nodes are equal when they say the same thing, wherever they were written.
impl PartialEq for PyExpr {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PyKind {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    None,
    /// A bare name: `pl`, `datetime`, or a keyword argument's value.
    Name(String),
    /// `receiver.attribute` — the dotted path polars is written in.
    Attr(Box<PyExpr>, String),
    /// `callee(args…)`, where `callee` is a name or an attribute chain.
    Call {
        callee: Box<PyExpr>,
        args: Vec<PyArg>,
    },
    /// `[a, b, c]`
    List(Vec<PyExpr>),
    Unary(PyUnop, Box<PyExpr>),
    Binary(PyBinop, Box<PyExpr>, Box<PyExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PyArg {
    pub name: Option<String>,
    pub value: PyExpr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyUnop {
    /// `-`
    Neg,
    /// `~`, which is how polars spells negation
    Invert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyBinop {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
}

impl PyBinop {
    /// Python's precedence, tightest first. The inverse of `emit::python`'s own
    /// table — the two must agree or emitted polars will not read back as
    /// itself. The one that matters: `&` and `|` bind **tighter** than
    /// comparison, the reverse of the language's `AND`/`OR`.
    pub fn prec(self) -> u8 {
        match self {
            PyBinop::Mul | PyBinop::Div | PyBinop::Mod => 5,
            PyBinop::Add | PyBinop::Sub => 4,
            PyBinop::BitAnd => 3,
            PyBinop::BitOr => 2,
            PyBinop::Eq | PyBinop::Ne | PyBinop::Lt | PyBinop::Le | PyBinop::Gt | PyBinop::Ge => 1,
        }
    }

    /// Whether this is one of the comparisons, which Python chains and the
    /// language does not.
    pub fn is_comparison(self) -> bool {
        self.prec() == 1
    }
}

/// The precedence of unary `-` and `~`, above every binary operator here.
pub const PREC_UNARY: u8 = 6;

impl PyExpr {
    /// The dotted name this is, if it is one: `pl.col` gives `"pl.col"`.
    pub fn dotted(&self) -> Option<String> {
        match &self.kind {
            PyKind::Name(name) => Some(name.clone()),
            PyKind::Attr(base, field) => Some(format!("{}.{field}", base.dotted()?)),
            _ => None,
        }
    }

    /// A call to the dotted name `callee`, giving its arguments.
    pub fn as_call(&self, callee: &str) -> Option<&[PyArg]> {
        match &self.kind {
            PyKind::Call { callee: c, args } if c.dotted().as_deref() == Some(callee) => Some(args),
            _ => None,
        }
    }

    /// A method call on some receiver: `x.name(args…)`, giving both.
    pub fn as_method(&self, name: &str) -> Option<(&PyExpr, &[PyArg])> {
        let PyKind::Call { callee, args } = &self.kind else {
            return None;
        };
        let PyKind::Attr(receiver, method) = &callee.kind else {
            return None;
        };
        (method == name).then_some((receiver.as_ref(), args))
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.kind {
            PyKind::Str(s) => Some(s),
            _ => None,
        }
    }
}
