//! Recursive-descent parser and semantic checker for the `assert` expression
//! mini-language used in column and table `constraints`.
//!
//! Grammar (precedence loosest to tightest, following standard SQL):
//!
//! ```text
//! expr        := or_expr
//! or_expr     := and_expr ("OR" and_expr)*
//! and_expr    := not_expr ("AND" not_expr)*
//! not_expr    := "NOT" not_expr | predicate
//! predicate   := additive ( cmp additive
//!                          | "IS" ["NOT"] "NULL"
//!                          | ["NOT"] "BETWEEN" additive "AND" additive
//!                          | ["NOT"] "IN" "(" expr ("," expr)* ")"
//!                          | ["NOT"] "LIKE" additive
//!                          | ["NOT"] "SIMILAR" "TO" additive )?
//! additive    := multiplicative (("+" | "-") multiplicative)*
//! multiplicative := unary (("*" | "/") unary)*
//! unary       := "-" unary | primary
//! primary     := literal | column | funcall | columns | case | "(" expr ")"
//! cmp         := "=" | "!=" | "<>" | "<" | "<=" | ">" | ">="
//! literal     := number | string | "TRUE" | "FALSE" | "NULL"
//! funcall     := IDENT "(" (expr ("," expr)*)? ")"   // incl. NOW(), interval(n, unit)
//! columns     := "COLUMNS" "(" ("*" | string | "[" column ("," column)* "]") ")"
//! case        := "CASE" ("WHEN" expr "THEN" expr)+ ("ELSE" expr)? "END"
//! column      := IDENT | QUOTED
//! IDENT       := [A-Za-z_][A-Za-z0-9_]*
//! QUOTED      := "`" ( [^`] | "``" )+ "`"
//! ```
//!
//! Keywords and function names are matched case-insensitively; column
//! identifiers are preserved verbatim (case-sensitive, matched against the
//! table). A column whose name isn't an `IDENT`, or which collides with a
//! reserved word, is written in backticks, doubling a backtick to embed one;
//! quoting affects only how the name is read, never how it is matched. String
//! literals are single-quoted, doubling a quote to embed one.
//! Every node records the byte offsets it spans within the input so diagnostics
//! can point at the failing token, exactly as [`crate::join_expr`] does.
//!
//! Parsing is pure syntax — it knows nothing about the table. Column
//! resolution, type checking, and the shape check live in [`check`], which walks
//! the parsed tree against a [`CheckEnv`] and emits the S20–S23/S30
//! [`Finding`]s.

/// A parsed assertion expression: the root node of the tree.
#[derive(Debug, Clone)]
pub struct AssertExpr {
    pub root: Expr,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    /// Byte offsets of this node within the assertion string.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Number {
        is_int: bool,
    },
    Str(String),
    Bool(bool),
    Null,
    /// A column reference, or a field of a `struct` column reached with dots:
    /// one segment per name, so `address.zip` is `["address", "zip"]`.
    Column(Vec<String>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Arith {
        op: ArithOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Compare {
        op: CmpOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    IsNull {
        operand: Box<Expr>,
        negated: bool,
    },
    Between {
        operand: Box<Expr>,
        lo: Box<Expr>,
        hi: Box<Expr>,
        negated: bool,
    },
    In {
        operand: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    Like {
        operand: Box<Expr>,
        pattern: Box<Expr>,
        negated: bool,
    },
    SimilarTo {
        operand: Box<Expr>,
        pattern: Box<Expr>,
        negated: bool,
    },
    /// A named function call other than `NOW`/`interval`; the name is preserved
    /// verbatim (classified case-insensitively during checking).
    Call {
        name: String,
        args: Vec<Expr>,
    },
    Now,
    Interval {
        n: Box<Expr>,
        unit: String,
        unit_start: usize,
        unit_end: usize,
    },
    Case {
        whens: Vec<(Expr, Expr)>,
        els: Option<Box<Expr>>,
    },
    Columns(ColumnsSelector),
}

#[derive(Debug, Clone)]
pub enum ColumnsSelector {
    All,
    /// A regex string with its byte span (for a regex-compile diagnostic).
    Regex {
        pattern: String,
        start: usize,
        end: usize,
    },
    /// An explicit list of column names, each with its byte span.
    List(Vec<Named>),
}

#[derive(Debug, Clone)]
pub struct Named {
    pub name: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

pub use crate::expr_lex::ParseError;

impl AssertExpr {
    pub fn parse(input: &str) -> Result<AssertExpr, ParseError> {
        let mut p = Parser::new(input);
        let root = p.parse_expr()?;
        p.skip_ws();
        if !p.is_eof() {
            return Err(p.err("unexpected trailing input"));
        }
        Ok(AssertExpr { root })
    }
}

// --- Parser ---------------------------------------------------------------

/// Reserved words that may not stand in for a column reference, so a stray
/// keyword where a term is expected fails cleanly rather than parsing as a
/// column named after a keyword.
const RESERVED: &[&str] = &[
    "and", "or", "not", "is", "null", "between", "in", "like", "similar", "to", "when", "then",
    "else", "end", "true", "false",
];

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            src: s.as_bytes(),
            pos: 0,
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError {
            message: msg.into(),
            at: self.pos,
        }
    }

    // --- expression levels ---

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while self.match_keyword("or") {
            let rhs = self.parse_and()?;
            let (start, end) = (lhs.start, rhs.end);
            lhs = Expr {
                kind: ExprKind::Or(Box::new(lhs), Box::new(rhs)),
                start,
                end,
            };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_not()?;
        while self.match_keyword("and") {
            let rhs = self.parse_not()?;
            let (start, end) = (lhs.start, rhs.end);
            lhs = Expr {
                kind: ExprKind::And(Box::new(lhs), Box::new(rhs)),
                start,
                end,
            };
        }
        Ok(lhs)
    }

    fn parse_not(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();
        let start = self.pos;
        if self.match_keyword("not") {
            let operand = self.parse_not()?;
            let end = operand.end;
            return Ok(Expr {
                kind: ExprKind::Not(Box::new(operand)),
                start,
                end,
            });
        }
        self.parse_predicate()
    }

    fn parse_predicate(&mut self) -> Result<Expr, ParseError> {
        let operand = self.parse_additive()?;
        self.skip_ws();

        if let Some(op) = self.try_cmp_op() {
            let rhs = self.parse_additive()?;
            let (start, end) = (operand.start, rhs.end);
            return Ok(Expr {
                kind: ExprKind::Compare {
                    op,
                    lhs: Box::new(operand),
                    rhs: Box::new(rhs),
                },
                start,
                end,
            });
        }

        if self.match_keyword("is") {
            let negated = self.match_keyword("not");
            self.expect_keyword("null")?;
            let (start, end) = (operand.start, self.pos);
            return Ok(Expr {
                kind: ExprKind::IsNull {
                    operand: Box::new(operand),
                    negated,
                },
                start,
                end,
            });
        }

        let negated = self.match_keyword("not");
        if self.match_keyword("between") {
            let lo = self.parse_additive()?;
            self.expect_keyword("and")?;
            let hi = self.parse_additive()?;
            let (start, end) = (operand.start, hi.end);
            return Ok(Expr {
                kind: ExprKind::Between {
                    operand: Box::new(operand),
                    lo: Box::new(lo),
                    hi: Box::new(hi),
                    negated,
                },
                start,
                end,
            });
        }
        if self.match_keyword("in") {
            self.skip_ws();
            self.expect_byte(b'(')?;
            let mut list = vec![self.parse_expr()?];
            loop {
                self.skip_ws();
                if self.try_byte(b',') {
                    list.push(self.parse_expr()?);
                } else {
                    break;
                }
            }
            self.skip_ws();
            self.expect_byte(b')')?;
            let (start, end) = (operand.start, self.pos);
            return Ok(Expr {
                kind: ExprKind::In {
                    operand: Box::new(operand),
                    list,
                    negated,
                },
                start,
                end,
            });
        }
        if self.match_keyword("like") {
            let pattern = self.parse_additive()?;
            let (start, end) = (operand.start, pattern.end);
            return Ok(Expr {
                kind: ExprKind::Like {
                    operand: Box::new(operand),
                    pattern: Box::new(pattern),
                    negated,
                },
                start,
                end,
            });
        }
        if self.match_keyword("similar") {
            self.expect_keyword("to")?;
            let pattern = self.parse_additive()?;
            let (start, end) = (operand.start, pattern.end);
            return Ok(Expr {
                kind: ExprKind::SimilarTo {
                    operand: Box::new(operand),
                    pattern: Box::new(pattern),
                    negated,
                },
                start,
                end,
            });
        }
        if negated {
            // A `NOT` here must introduce one of the infix predicates above.
            return Err(self.err("expected `BETWEEN`, `IN`, `LIKE`, or `SIMILAR TO` after `NOT`"));
        }
        Ok(operand)
    }

    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_multiplicative()?;
        loop {
            self.skip_ws();
            let op = match self.peek() {
                Some(b'+') => ArithOp::Add,
                Some(b'-') => ArithOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_multiplicative()?;
            let (start, end) = (lhs.start, rhs.end);
            lhs = Expr {
                kind: ExprKind::Arith {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                start,
                end,
            };
        }
        Ok(lhs)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            self.skip_ws();
            let op = match self.peek() {
                Some(b'*') => ArithOp::Mul,
                Some(b'/') => ArithOp::Div,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_unary()?;
            let (start, end) = (lhs.start, rhs.end);
            lhs = Expr {
                kind: ExprKind::Arith {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                start,
                end,
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
            let operand = self.parse_unary()?;
            let end = operand.end;
            return Ok(Expr {
                kind: ExprKind::Neg(Box::new(operand)),
                start,
                end,
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();
        let start = self.pos;
        match self.peek() {
            None => Err(self.err("expected an expression")),
            Some(b'(') => {
                self.pos += 1;
                let inner = self.parse_expr()?;
                self.skip_ws();
                self.expect_byte(b')')?;
                // Re-span so the node covers the parentheses.
                Ok(Expr {
                    kind: inner.kind,
                    start,
                    end: self.pos,
                })
            }
            Some(b'\'') => self.parse_string(),
            Some(b'`') => {
                let name = crate::expr_lex::parse_quoted_name(self.src, &mut self.pos)?;
                let path = self.parse_field_segments(name)?;
                Ok(self.node(ExprKind::Column(path), start))
            }
            Some(b) if b.is_ascii_digit() => self.parse_number(),
            Some(b) if b.is_ascii_alphabetic() || b == b'_' => self.parse_word_expr(),
            _ => Err(self.err("expected an expression")),
        }
    }

    fn parse_string(&mut self) -> Result<Expr, ParseError> {
        let start = self.pos;
        debug_assert_eq!(self.peek(), Some(b'\''));
        self.pos += 1;
        let mut value = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err("unterminated string literal")),
                Some(b'\'') => {
                    // A doubled quote is a literal quote; a lone one ends it.
                    if self.src.get(self.pos + 1) == Some(&b'\'') {
                        value.push('\'');
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                        break;
                    }
                }
                Some(_) => {
                    let ch_start = self.pos;
                    crate::expr_lex::advance_char(self.src, &mut self.pos);
                    value.push_str(
                        std::str::from_utf8(&self.src[ch_start..self.pos])
                            .expect("input is valid utf-8"),
                    );
                }
            }
        }
        Ok(Expr {
            kind: ExprKind::Str(value),
            start,
            end: self.pos,
        })
    }

    fn parse_number(&mut self) -> Result<Expr, ParseError> {
        let start = self.pos;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        let mut is_int = true;
        if self.peek() == Some(b'.') && self.src.get(self.pos + 1).is_some_and(u8::is_ascii_digit) {
            is_int = false;
            self.pos += 1;
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        Ok(Expr {
            kind: ExprKind::Number { is_int },
            start,
            end: self.pos,
        })
    }

    /// Parse a word-led primary: a keyword literal, a `COLUMNS`/`CASE`/`NOW`/
    /// `interval` construct, a function call, or a bare column reference.
    fn parse_word_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.pos;
        let word = self.read_word();
        let lower = word.to_ascii_lowercase();
        match lower.as_str() {
            "true" => Ok(self.node(ExprKind::Bool(true), start)),
            "false" => Ok(self.node(ExprKind::Bool(false), start)),
            "null" => Ok(self.node(ExprKind::Null, start)),
            "case" => self.parse_case(start),
            "columns" => self.parse_columns(start),
            "now" => {
                self.skip_ws();
                self.expect_byte(b'(')?;
                self.skip_ws();
                self.expect_byte(b')')?;
                Ok(self.node(ExprKind::Now, start))
            }
            "interval" => self.parse_interval(start),
            _ => {
                if RESERVED.contains(&lower.as_str()) {
                    return Err(ParseError {
                        message: format!("unexpected keyword `{}`", word.to_uppercase()),
                        at: start,
                    });
                }
                // A `(` immediately (ignoring whitespace) after the word makes
                // it a function call; otherwise it is a column reference.
                let after = self.pos;
                self.skip_ws();
                if self.peek() == Some(b'(') {
                    self.pos += 1;
                    let args = self.parse_arg_list()?;
                    Ok(self.node(ExprKind::Call { name: word, args }, start))
                } else {
                    self.pos = after;
                    let path = self.parse_field_segments(word)?;
                    Ok(self.node(ExprKind::Column(path), start))
                }
            }
        }
    }

    fn parse_arg_list(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        self.skip_ws();
        if self.try_byte(b')') {
            return Ok(args);
        }
        args.push(self.parse_expr()?);
        loop {
            self.skip_ws();
            if self.try_byte(b',') {
                args.push(self.parse_expr()?);
            } else {
                break;
            }
        }
        self.skip_ws();
        self.expect_byte(b')')?;
        Ok(args)
    }

    fn parse_interval(&mut self, start: usize) -> Result<Expr, ParseError> {
        self.skip_ws();
        self.expect_byte(b'(')?;
        let n = self.parse_expr()?;
        self.skip_ws();
        self.expect_byte(b',')?;
        self.skip_ws();
        let unit_start = self.pos;
        if !self
            .peek()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        {
            return Err(self.err("expected an interval unit"));
        }
        let unit = self.read_word();
        let unit_end = self.pos;
        self.skip_ws();
        self.expect_byte(b')')?;
        Ok(self.node(
            ExprKind::Interval {
                n: Box::new(n),
                unit,
                unit_start,
                unit_end,
            },
            start,
        ))
    }

    fn parse_columns(&mut self, start: usize) -> Result<Expr, ParseError> {
        self.skip_ws();
        self.expect_byte(b'(')?;
        self.skip_ws();
        let selector = match self.peek() {
            Some(b'*') => {
                self.pos += 1;
                ColumnsSelector::All
            }
            Some(b'\'') => {
                let s = self.parse_string()?;
                let ExprKind::Str(pattern) = s.kind else {
                    unreachable!("parse_string yields a Str")
                };
                ColumnsSelector::Regex {
                    pattern,
                    start: s.start,
                    end: s.end,
                }
            }
            Some(b'[') => {
                self.pos += 1;
                let mut names = Vec::new();
                loop {
                    self.skip_ws();
                    let n_start = self.pos;
                    let name = match self.peek() {
                        Some(b'`') => crate::expr_lex::parse_quoted_name(self.src, &mut self.pos)?,
                        Some(b) if b.is_ascii_alphabetic() || b == b'_' => self.read_word(),
                        _ => return Err(self.err("expected a column name")),
                    };
                    names.push(Named {
                        name,
                        start: n_start,
                        end: self.pos,
                    });
                    self.skip_ws();
                    if self.try_byte(b',') {
                        continue;
                    }
                    self.expect_byte(b']')?;
                    break;
                }
                ColumnsSelector::List(names)
            }
            _ => return Err(self.err("expected `*`, a regex string, or `[names]`")),
        };
        self.skip_ws();
        self.expect_byte(b')')?;
        Ok(self.node(ExprKind::Columns(selector), start))
    }

    fn parse_case(&mut self, start: usize) -> Result<Expr, ParseError> {
        let mut whens = Vec::new();
        while self.match_keyword("when") {
            let cond = self.parse_expr()?;
            self.expect_keyword("then")?;
            let result = self.parse_expr()?;
            whens.push((cond, result));
        }
        if whens.is_empty() {
            return Err(self.err("`CASE` needs at least one `WHEN ... THEN ...`"));
        }
        let els = if self.match_keyword("else") {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect_keyword("end")?;
        Ok(self.node(ExprKind::Case { whens, els }, start))
    }

    /// Extend a just-parsed column name into a field path: each `.` that
    /// immediately follows (no whitespace, like the name itself) must be
    /// followed by another name — bare and unreserved, or backtick-quoted.
    fn parse_field_segments(&mut self, first: String) -> Result<Vec<String>, ParseError> {
        let mut path = vec![first];
        while self.peek() == Some(b'.') {
            self.pos += 1;
            match self.peek() {
                Some(b'`') => {
                    path.push(crate::expr_lex::parse_quoted_name(self.src, &mut self.pos)?);
                }
                Some(b) if b.is_ascii_alphabetic() || b == b'_' => {
                    let at = self.pos;
                    let word = self.read_word();
                    if RESERVED.contains(&word.to_ascii_lowercase().as_str()) {
                        return Err(ParseError {
                            message: format!("unexpected keyword `{}`", word.to_uppercase()),
                            at,
                        });
                    }
                    path.push(word);
                }
                _ => return Err(self.err("expected a field name after `.`")),
            }
        }
        Ok(path)
    }

    // --- token helpers ---

    fn node(&self, kind: ExprKind, start: usize) -> Expr {
        Expr {
            kind,
            start,
            end: self.pos,
        }
    }

    fn read_word(&mut self) -> String {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.src[start..self.pos])
            .expect("identifier bytes are ASCII")
            .to_string()
    }

    fn try_cmp_op(&mut self) -> Option<CmpOp> {
        // Order matters: two-character operators before their prefixes.
        for (lit, op) in [
            (">=", CmpOp::Ge),
            ("<=", CmpOp::Le),
            ("<>", CmpOp::Ne),
            ("!=", CmpOp::Ne),
            ("=", CmpOp::Eq),
            (">", CmpOp::Gt),
            ("<", CmpOp::Lt),
        ] {
            if self.src[self.pos..].starts_with(lit.as_bytes()) {
                self.pos += lit.len();
                return Some(op);
            }
        }
        None
    }

    fn try_byte(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_byte(&mut self, b: u8) -> Result<(), ParseError> {
        if self.try_byte(b) {
            Ok(())
        } else {
            Err(self.err(format!("expected `{}`", b as char)))
        }
    }

    /// Consume `kw` (case-insensitive) if it appears next as a whole word,
    /// returning whether it did. A trailing identifier character blocks the
    /// match so `interval` is not seen inside `intervals`.
    fn match_keyword(&mut self, kw: &str) -> bool {
        let save = self.pos;
        self.skip_ws();
        let end = self.pos + kw.len();
        if end > self.src.len() || !self.src[self.pos..end].eq_ignore_ascii_case(kw.as_bytes()) {
            self.pos = save;
            return false;
        }
        if self
            .src
            .get(end)
            .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_')
        {
            self.pos = save;
            return false;
        }
        self.pos = end;
        true
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), ParseError> {
        if self.match_keyword(kw) {
            Ok(())
        } else {
            Err(self.err(format!("expected `{}`", kw.to_uppercase())))
        }
    }
}

// --- Semantic checking (S20 / S21) ----------------------------------------

/// The kind a column resolves to for type checking. An `enum` resolves to the
/// kind of its values; `Untyped` is a column the dictionary says nothing about,
/// which can't be used where a type matters (S23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Number,
    String,
    Bool,
    Date,
    Datetime,
    /// A `struct` column or field: no scalar value of its own, but its fields
    /// are reachable with dot access.
    Struct,
    /// A `list(...)` column or field: no scalar value, and its elements can't
    /// be reached in a per-row expression.
    List,
    Untyped,
}

/// What an assertion checker needs to know about the table it runs against.
pub trait CheckEnv {
    /// The kind of column `name`, or `None` if the table has no such column.
    fn column(&self, name: &str) -> Option<ColumnKind>;
    /// The kind of the field reached by `path` — a column name followed by one
    /// or more field names — or `None` if any segment doesn't exist. The env
    /// only looks names up; that each intermediate segment is a `struct` is the
    /// checker's to enforce.
    fn field(&self, path: &[String]) -> Option<ColumnKind>;
    /// Every column on the table, in declaration order, with its kind. Used to
    /// resolve a `COLUMNS(...)` selection to the columns it matches.
    fn columns(&self) -> Vec<(String, ColumnKind)>;
    /// Whether `s` parses as an ISO 8601 date.
    fn is_date(&self, s: &str) -> bool;
    /// Whether `s` parses as an ISO 8601 datetime (offset or zoneless).
    fn is_datetime(&self, s: &str) -> bool;
}

/// One problem found in an assertion, with its byte span in the source
/// expression. `code` is `"S20"` (unknown column), `"S21"` (ill-typed), `"S22"`
/// (empty column selection, a warning), `"S23"` (untyped column), or `"S30"`
/// (nested aggregate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub code: &'static str,
    pub severity: FindingSeverity,
    pub message: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingSeverity {
    Error,
    Warning,
}

/// The inferred type of a subexpression. Two variants are not real types, and
/// they are opposites: `Any` is the permissive top (`NULL`, or a subexpression
/// already reported as wrong) and is compatible with everything, so a single
/// root cause yields a single diagnostic; `Unknown` is a column with no
/// declared `type`, and is compatible with nothing — using it where a type
/// matters is S23.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Number,
    String,
    Bool,
    Date,
    Datetime,
    Interval,
    /// A `struct` or `list` column has no scalar value: it may stand bare only
    /// where no type is needed (`IS [NOT] NULL`), so these two satisfy no
    /// requirement and compare with nothing.
    Struct,
    List,
    Any,
    Unknown,
}

impl Ty {
    fn noun(self) -> &'static str {
        match self {
            Ty::Number => "a number",
            Ty::String => "a string",
            Ty::Bool => "a boolean",
            Ty::Date => "a date",
            Ty::Datetime => "a datetime",
            Ty::Interval => "an interval",
            Ty::Struct => "a struct",
            Ty::List => "a list",
            Ty::Any => "a value",
            Ty::Unknown => "a value of unknown type",
        }
    }
}

fn kind_to_ty(kind: ColumnKind) -> Ty {
    match kind {
        ColumnKind::Number => Ty::Number,
        ColumnKind::String => Ty::String,
        ColumnKind::Bool => Ty::Bool,
        ColumnKind::Date => Ty::Date,
        ColumnKind::Datetime => Ty::Datetime,
        ColumnKind::Struct => Ty::Struct,
        ColumnKind::List => Ty::List,
        ColumnKind::Untyped => Ty::Unknown,
    }
}

/// The nouns of `types` as a prose list: "a number", "a number or a string",
/// "a number, a string, or a date".
fn join_nouns(types: &[Ty]) -> String {
    let nouns: Vec<&str> = types.iter().map(|t| t.noun()).collect();
    match nouns.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [a, b] => format!("{a} or {b}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

/// How many values an expression stands for. The variants are ordered so that
/// `max` implements the rule that an operator takes the largest shape among its
/// operands: `Const` is the identity, and `Row` absorbs `Agg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Shape {
    Const,
    Agg,
    Row,
}

/// The types a function's argument may take.
#[derive(Clone, Copy)]
enum ArgClass {
    Only(&'static [Ty]),
    /// `COUNT`, which asks only whether a value is null. Nothing about the
    /// argument's type matters — not even that it has one — so a `struct`, a
    /// `list`, and an untyped column are all accepted, as they are by `IS NULL`.
    Unconstrained,
}

/// What a function returns: a fixed type, or whatever its first argument was
/// (`MIN`/`MAX`, the language's only parametric signatures).
#[derive(Clone, Copy)]
enum Ret {
    Fixed(Ty),
    SameAsArg,
}

#[derive(Clone, Copy)]
struct Sig {
    arities: &'static [usize],
    arg: ArgClass,
    ret: Ret,
    aggregate: bool,
}

impl Sig {
    const fn scalar(arities: &'static [usize], arg: &'static [Ty], ret: Ret) -> Sig {
        Sig {
            arities,
            arg: ArgClass::Only(arg),
            ret,
            aggregate: false,
        }
    }

    const fn agg(arities: &'static [usize], arg: &'static [Ty], ret: Ret) -> Sig {
        Sig {
            arities,
            arg: ArgClass::Only(arg),
            ret,
            aggregate: true,
        }
    }
}

/// The signature of the named function, or `None` if there is no such function.
/// `name` must already be lowercased. `NOW` and `interval` are absent: the
/// parser gives them their own AST nodes.
fn signature(name: &str) -> Option<Sig> {
    use Ret::{Fixed, SameAsArg};
    const STRING: &[Ty] = &[Ty::String];
    const NUMBER: &[Ty] = &[Ty::Number];
    const BOOL: &[Ty] = &[Ty::Bool];
    const ORDERED: &[Ty] = &[Ty::Number, Ty::String, Ty::Date, Ty::Datetime];

    Some(match name {
        "length" => Sig::scalar(&[1], STRING, Fixed(Ty::Number)),
        "lower" | "upper" | "trim" => Sig::scalar(&[1], STRING, Fixed(Ty::String)),
        "starts_with" | "ends_with" => Sig::scalar(&[2], STRING, Fixed(Ty::Bool)),
        "abs" | "floor" | "ceil" => Sig::scalar(&[1], NUMBER, Fixed(Ty::Number)),
        "round" => Sig::scalar(&[1, 2], NUMBER, Fixed(Ty::Number)),
        "mod" => Sig::scalar(&[2], NUMBER, Fixed(Ty::Number)),
        "min" | "max" => Sig::agg(&[1], ORDERED, SameAsArg),
        "sum" | "avg" => Sig::agg(&[1], NUMBER, Fixed(Ty::Number)),
        "count_distinct" => Sig::agg(&[1], ORDERED, Fixed(Ty::Number)),
        "any" | "all" => Sig::agg(&[1], BOOL, Fixed(Ty::Bool)),
        "count" => Sig {
            arities: &[1],
            arg: ArgClass::Unconstrained,
            ret: Fixed(Ty::Number),
            aggregate: true,
        },
        "row_count" => Sig {
            arities: &[0],
            arg: ArgClass::Unconstrained,
            ret: Fixed(Ty::Number),
            aggregate: true,
        },
        _ => return None,
    })
}

/// Check a parsed assertion against `env`, returning every finding in source
/// order. The expression must evaluate to a boolean, at most one `COLUMNS(...)`
/// may appear, and every operand whose type matters must have a known one.
pub fn check(expr: &AssertExpr, env: &dyn CheckEnv) -> Vec<Finding> {
    let mut cx = Checker {
        env,
        findings: Vec::new(),
        columns_spans: Vec::new(),
    };
    let ty = cx.infer(&expr.root);
    cx.shape(&expr.root);
    // The assertion as a whole must be boolean. A bare top-level COLUMNS(...)
    // stands for each selected column, so every one of those must be boolean.
    if let ExprKind::Columns(sel) = &expr.root.kind {
        cx.require_columns(&expr.root, sel, &[Ty::Bool], "an assertion");
    } else if ty == Ty::Unknown {
        cx.report_unknown(&expr.root);
    } else if !matches!(ty, Ty::Bool | Ty::Any) {
        cx.report(
            "S21",
            format!("this assertion is {}, not a boolean", ty.noun()),
            &expr.root,
        );
    }
    // At most one COLUMNS(...) may appear; flag every one past the first.
    if cx.columns_spans.len() > 1 {
        for &(start, end) in &cx.columns_spans[1..] {
            cx.findings.push(Finding {
                code: "S21",
                severity: FindingSeverity::Error,
                message: "an assertion may use at most one `COLUMNS(...)`".to_string(),
                start,
                end,
            });
        }
    }
    cx.findings.sort_by_key(|f| (f.start, f.end));
    // A node can be visited twice — inferred, then required by its parent — so
    // the same fault can be recorded twice.
    cx.findings.dedup();
    cx.findings
}

struct Checker<'a> {
    env: &'a dyn CheckEnv,
    findings: Vec<Finding>,
    columns_spans: Vec<(usize, usize)>,
}

impl Checker<'_> {
    fn report(&mut self, code: &'static str, message: impl Into<String>, e: &Expr) {
        self.findings.push(Finding {
            code,
            severity: FindingSeverity::Error,
            message: message.into(),
            start: e.start,
            end: e.end,
        });
    }

    /// Report the S23 for a value used where its type matters but isn't known.
    fn report_unknown(&mut self, e: &Expr) {
        let message = match &e.kind {
            ExprKind::Column(path) if path.len() > 1 => {
                format!("field `{}` has no declared type", path.join("."))
            }
            ExprKind::Column(path) => format!("column `{}` has no declared type", path[0]),
            _ => "this value's type is unknown".to_string(),
        };
        self.report("S23", message, e);
    }

    /// Require `e` to have a type in `allowed` (with `Any` always accepted),
    /// reporting an S21 against `e` naming `ctx` if not, or an S23 if `e`'s type
    /// isn't known at all. A `COLUMNS(...)` operand is checked per selected
    /// column, since the predicate applies to each.
    ///
    /// Returns `e`'s inferred type, so a caller that needs it doesn't visit `e`
    /// a second time — a second visit would double-count a `COLUMNS(...)`.
    fn require(&mut self, e: &Expr, allowed: &[Ty], ctx: &str) -> Ty {
        if let ExprKind::Columns(sel) = &e.kind {
            let ty = self.infer(e);
            self.require_columns(e, sel, allowed, ctx);
            return ty;
        }
        let ty = self.infer(e);
        if ty == Ty::Unknown {
            self.report_unknown(e);
        } else if ty != Ty::Any && !allowed.contains(&ty) {
            self.report(
                "S21",
                format!("{ctx} expects {}, found {}", join_nouns(allowed), ty.noun()),
                e,
            );
        }
        ty
    }

    /// Require every column a `COLUMNS(...)` node selects to satisfy `allowed`.
    fn require_columns(&mut self, cols: &Expr, sel: &ColumnsSelector, allowed: &[Ty], ctx: &str) {
        for (name, kind) in self.matched_columns(sel) {
            let ty = kind_to_ty(kind);
            if ty == Ty::Unknown {
                self.report("S23", format!("column `{name}` has no declared type"), cols);
            } else if ty != Ty::Any && !allowed.contains(&ty) {
                self.report(
                    "S21",
                    format!(
                        "{ctx} expects {}, but column `{name}` is {}",
                        join_nouns(allowed),
                        ty.noun()
                    ),
                    cols,
                );
            }
        }
    }

    fn infer(&mut self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Number { .. } => Ty::Number,
            ExprKind::Str(_) => Ty::String,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Null => Ty::Any,
            ExprKind::Column(path) => self.infer_column_path(path, e),
            ExprKind::Neg(inner) => {
                self.require(inner, &[Ty::Number], "negation");
                Ty::Number
            }
            ExprKind::Not(inner) => {
                self.require(inner, &[Ty::Bool], "`NOT`");
                Ty::Bool
            }
            ExprKind::And(l, r) | ExprKind::Or(l, r) => {
                self.require(l, &[Ty::Bool], "a logical operator");
                self.require(r, &[Ty::Bool], "a logical operator");
                Ty::Bool
            }
            ExprKind::Arith { op, lhs, rhs } => self.infer_arith(*op, lhs, rhs),
            ExprKind::Compare { lhs, rhs, .. } => {
                self.check_comparable(lhs, rhs);
                Ty::Bool
            }
            ExprKind::IsNull { operand, .. } => {
                self.infer(operand);
                Ty::Bool
            }
            ExprKind::Between {
                operand, lo, hi, ..
            } => {
                self.check_comparable(operand, lo);
                self.check_comparable(operand, hi);
                Ty::Bool
            }
            ExprKind::In { operand, list, .. } => {
                for item in list {
                    self.check_comparable(operand, item);
                }
                Ty::Bool
            }
            ExprKind::Like {
                operand, pattern, ..
            } => {
                self.require(operand, &[Ty::String], "`LIKE`");
                self.require(pattern, &[Ty::String], "a `LIKE` pattern");
                Ty::Bool
            }
            ExprKind::SimilarTo {
                operand, pattern, ..
            } => {
                self.require(operand, &[Ty::String], "`SIMILAR TO`");
                self.require(pattern, &[Ty::String], "a `SIMILAR TO` pattern");
                if let ExprKind::Str(pat) = &pattern.kind {
                    self.check_regex(pat, pattern);
                }
                Ty::Bool
            }
            ExprKind::Now => Ty::Datetime,
            ExprKind::Interval {
                n,
                unit,
                unit_start,
                unit_end,
            } => {
                self.require(n, &[Ty::Number], "`interval`");
                const UNITS: &[&str] = &["seconds", "minutes", "hours", "days", "weeks"];
                if !UNITS.contains(&unit.to_ascii_lowercase().as_str()) {
                    self.findings.push(Finding {
                        code: "S21",
                        severity: FindingSeverity::Error,
                        message: format!(
                            "`{unit}` is not an interval unit (use seconds, minutes, hours, days, or weeks)"
                        ),
                        start: *unit_start,
                        end: *unit_end,
                    });
                }
                Ty::Interval
            }
            ExprKind::Call { name, args } => self.infer_call(name, args, e),
            ExprKind::Case { whens, els } => self.infer_case(whens, els.as_deref()),
            ExprKind::Columns(sel) => {
                self.columns_spans.push((e.start, e.end));
                self.validate_selector(sel, e);
                Ty::Any
            }
        }
    }

    fn infer_arith(&mut self, op: ArithOp, lhs: &Expr, rhs: &Expr) -> Ty {
        let lt = self.infer(lhs);
        let rt = self.infer(rhs);
        // An unknown operand decides nothing about the result, so report it here
        // rather than letting the numeric path below blame the other operand.
        if lt == Ty::Unknown || rt == Ty::Unknown {
            if lt == Ty::Unknown {
                self.report_unknown(lhs);
            }
            if rt == Ty::Unknown {
                self.report_unknown(rhs);
            }
            return Ty::Any;
        }
        // Date/datetime plus or minus an interval yields the same date kind.
        if matches!(op, ArithOp::Add | ArithOp::Sub) {
            for (date_ty, other_ty) in [(lt, rt), (rt, lt)] {
                if matches!(date_ty, Ty::Date | Ty::Datetime)
                    && matches!(other_ty, Ty::Interval | Ty::Any)
                {
                    return date_ty;
                }
            }
        }
        // Otherwise it is ordinary numeric arithmetic.
        self.require(lhs, &[Ty::Number], "arithmetic");
        self.require(rhs, &[Ty::Number], "arithmetic");
        Ty::Number
    }

    /// Check that `a` and `b` may be compared. When one side is a `COLUMNS(...)`
    /// selection, each selected column must be comparable with the other side.
    /// Resolve a column path to its type, walking one field per dot. Each
    /// resolution failure has its own report: an unknown column or field is
    /// S20, a dot applied to anything that isn't a `struct` is S21. `Any` comes
    /// back after a report so the one root cause yields one diagnostic.
    fn infer_column_path(&mut self, path: &[String], e: &Expr) -> Ty {
        let mut kind = match self.env.column(&path[0]) {
            Some(kind) => kind,
            None => {
                let name = &path[0];
                self.report("S20", format!("column `{name}` is not on this table"), e);
                return Ty::Any;
            }
        };
        for (i, segment) in path.iter().enumerate().skip(1) {
            let prefix = path[..i].join(".");
            match kind {
                ColumnKind::Struct => {}
                ColumnKind::List => {
                    self.report(
                        "S21",
                        format!("`{prefix}` is a list, and a list's elements can't be reached"),
                        e,
                    );
                    return Ty::Any;
                }
                other => {
                    let noun = kind_to_ty(other).noun();
                    self.report("S21", format!("`{prefix}` is {noun}, not a struct"), e);
                    return Ty::Any;
                }
            }
            kind = match self.env.field(&path[..=i]) {
                Some(kind) => kind,
                None => {
                    self.report(
                        "S20",
                        format!("struct `{prefix}` has no field `{segment}`"),
                        e,
                    );
                    return Ty::Any;
                }
            };
        }
        kind_to_ty(kind)
    }

    fn check_comparable(&mut self, a: &Expr, b: &Expr) {
        if let ExprKind::Columns(sel) = &a.kind {
            self.infer(a);
            self.compare_columns(a, sel, b);
            return;
        }
        if let ExprKind::Columns(sel) = &b.kind {
            self.infer(b);
            self.compare_columns(b, sel, a);
            return;
        }
        let at = self.infer(a);
        let bt = self.infer(b);
        if at == Ty::Unknown || bt == Ty::Unknown {
            if at == Ty::Unknown {
                self.report_unknown(a);
            }
            if bt == Ty::Unknown {
                self.report_unknown(b);
            }
            return;
        }
        if !self.types_comparable(at, a, bt, b) {
            self.report(
                "S21",
                format!("cannot compare {} with {}", at.noun(), bt.noun()),
                b,
            );
        }
    }

    /// Each column a `COLUMNS(...)` node selects must be comparable with `other`.
    fn compare_columns(&mut self, cols: &Expr, sel: &ColumnsSelector, other: &Expr) {
        let ot = self.infer(other);
        if ot == Ty::Unknown {
            self.report_unknown(other);
            return;
        }
        for (name, kind) in self.matched_columns(sel) {
            let ct = kind_to_ty(kind);
            if ct == Ty::Unknown {
                self.report("S23", format!("column `{name}` has no declared type"), cols);
            } else if !self.types_comparable(ct, cols, ot, other) {
                self.report(
                    "S21",
                    format!(
                        "column `{name}` ({}) cannot be compared with {}",
                        ct.noun(),
                        ot.noun()
                    ),
                    cols,
                );
            }
        }
    }

    /// Two types are comparable when they agree, either is permissive, both are
    /// temporal (a `date` against `NOW()`), or one operand is a string literal
    /// naming the date/datetime the other side is.
    fn types_comparable(&self, at: Ty, a: &Expr, bt: Ty, b: &Expr) -> bool {
        // A struct or list has no scalar value, so it compares with nothing —
        // not even another struct. `IS [NOT] NULL` is the only test it takes.
        if matches!(at, Ty::Struct | Ty::List) || matches!(bt, Ty::Struct | Ty::List) {
            return false;
        }
        at == Ty::Any
            || bt == Ty::Any
            || at == bt
            || (matches!(at, Ty::Date | Ty::Datetime) && matches!(bt, Ty::Date | Ty::Datetime))
            || self.date_literal_ok(a, at, bt)
            || self.date_literal_ok(b, bt, at)
    }

    /// True when `lit` is a string literal whose text parses as `other_ty`, a
    /// date or datetime, so `birthdate >= '2000-01-01'` is allowed.
    fn date_literal_ok(&self, lit: &Expr, lit_ty: Ty, other_ty: Ty) -> bool {
        if lit_ty != Ty::String {
            return false;
        }
        let ExprKind::Str(s) = &lit.kind else {
            return false;
        };
        match other_ty {
            Ty::Date => self.env.is_date(s),
            Ty::Datetime => self.env.is_datetime(s),
            _ => false,
        }
    }

    fn check_regex(&mut self, pattern: &str, e: &Expr) {
        if let Err(err) = regex::Regex::new(pattern) {
            let detail = err.to_string();
            let first = detail.lines().next().unwrap_or("invalid regex");
            self.report("S21", format!("invalid regular expression: {first}"), e);
        }
    }

    /// Validate a `COLUMNS(...)` selector itself (independent of how its result
    /// is used): the regex must compile (S21), listed names must exist (S20), and
    /// a regex matching no columns is a likely-dead selection (S22, a warning).
    fn validate_selector(&mut self, sel: &ColumnsSelector, cols: &Expr) {
        match sel {
            ColumnsSelector::All => {}
            ColumnsSelector::Regex {
                pattern,
                start,
                end,
            } => match regex::Regex::new(pattern) {
                Err(err) => {
                    let detail = err.to_string();
                    let first = detail.lines().next().unwrap_or("invalid regex");
                    self.findings.push(Finding {
                        code: "S21",
                        severity: FindingSeverity::Error,
                        message: format!("invalid regular expression: {first}"),
                        start: *start,
                        end: *end,
                    });
                }
                Ok(re) => {
                    if !self.env.columns().iter().any(|(n, _)| re.is_match(n)) {
                        self.findings.push(Finding {
                            code: "S22",
                            severity: FindingSeverity::Warning,
                            message: format!(
                                "`COLUMNS('{pattern}')` matches no columns on this table"
                            ),
                            start: cols.start,
                            end: cols.end,
                        });
                    }
                }
            },
            ColumnsSelector::List(names) => {
                for n in names {
                    if self.env.column(&n.name).is_none() {
                        self.findings.push(Finding {
                            code: "S20",
                            severity: FindingSeverity::Error,
                            message: format!("column `{}` is not on this table", n.name),
                            start: n.start,
                            end: n.end,
                        });
                    }
                }
            }
        }
    }

    /// The columns a selector matches, with their kinds. A regex that fails to
    /// compile (already reported) matches nothing; unknown list names (already
    /// reported) are skipped.
    fn matched_columns(&self, sel: &ColumnsSelector) -> Vec<(String, ColumnKind)> {
        match sel {
            ColumnsSelector::All => self.env.columns(),
            ColumnsSelector::Regex { pattern, .. } => match regex::Regex::new(pattern) {
                Ok(re) => self
                    .env
                    .columns()
                    .into_iter()
                    .filter(|(n, _)| re.is_match(n))
                    .collect(),
                Err(_) => Vec::new(),
            },
            ColumnsSelector::List(names) => names
                .iter()
                .filter_map(|n| self.env.column(&n.name).map(|k| (n.name.clone(), k)))
                .collect(),
        }
    }

    fn infer_call(&mut self, name: &str, args: &[Expr], e: &Expr) -> Ty {
        let lower = name.to_ascii_lowercase();
        let Some(sig) = signature(&lower) else {
            self.report("S21", format!("unknown function `{name}`"), e);
            for a in args {
                self.infer(a);
            }
            return Ty::Any;
        };
        if !sig.arities.contains(&args.len()) {
            let want = sig
                .arities
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(" or ");
            self.report(
                "S21",
                format!(
                    "`{}` takes {want} argument(s), found {}",
                    lower.to_uppercase(),
                    args.len()
                ),
                e,
            );
        }
        let ctx = format!("`{}`", lower.to_uppercase());
        let mut first: Option<Ty> = None;
        for (i, a) in args.iter().enumerate() {
            let ty = match sig.arg {
                ArgClass::Only(allowed) => self.require(a, allowed, &ctx),
                ArgClass::Unconstrained => self.infer(a),
            };
            if i == 0 {
                first = Some(ty);
            }
        }
        match (sig.ret, sig.arg) {
            (Ret::Fixed(t), _) => t,
            // An argument outside the class is already reported, and a
            // `COLUMNS(...)` argument infers to `Any`; either way `Any` keeps one
            // root cause to one diagnostic.
            (Ret::SameAsArg, ArgClass::Only(allowed)) => match first {
                Some(t) if allowed.contains(&t) => t,
                _ => Ty::Any,
            },
            (Ret::SameAsArg, ArgClass::Unconstrained) => Ty::Any,
        }
    }

    /// Compute `e`'s [`Shape`], reporting S30 for an aggregate applied to an
    /// argument that is itself aggregated. Shape follows from syntax alone, so
    /// this is a separate walk from [`Checker::infer`].
    fn shape(&mut self, e: &Expr) -> Shape {
        match &e.kind {
            ExprKind::Number { .. }
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Now => Shape::Const,
            ExprKind::Column(_) | ExprKind::Columns(_) => Shape::Row,
            ExprKind::Neg(inner) | ExprKind::Not(inner) => self.shape(inner),
            ExprKind::IsNull { operand, .. } => self.shape(operand),
            ExprKind::Interval { n, .. } => self.shape(n),
            ExprKind::Arith { lhs, rhs, .. } | ExprKind::Compare { lhs, rhs, .. } => {
                let l = self.shape(lhs);
                l.max(self.shape(rhs))
            }
            ExprKind::And(l, r) | ExprKind::Or(l, r) => {
                let ls = self.shape(l);
                ls.max(self.shape(r))
            }
            ExprKind::Like {
                operand, pattern, ..
            }
            | ExprKind::SimilarTo {
                operand, pattern, ..
            } => {
                let o = self.shape(operand);
                o.max(self.shape(pattern))
            }
            ExprKind::Between {
                operand, lo, hi, ..
            } => {
                let mut s = self.shape(operand);
                s = s.max(self.shape(lo));
                s.max(self.shape(hi))
            }
            ExprKind::In { operand, list, .. } => {
                let mut s = self.shape(operand);
                for item in list {
                    s = s.max(self.shape(item));
                }
                s
            }
            ExprKind::Case { whens, els } => {
                let mut s = Shape::Const;
                for (cond, result) in whens {
                    s = s.max(self.shape(cond));
                    s = s.max(self.shape(result));
                }
                if let Some(els) = els {
                    s = s.max(self.shape(els));
                }
                s
            }
            ExprKind::Call { name, args } => {
                let aggregate =
                    signature(&name.to_ascii_lowercase()).is_some_and(|sig| sig.aggregate);
                let mut widest = Shape::Const;
                for a in args {
                    let s = self.shape(a);
                    if aggregate && s == Shape::Agg {
                        self.report(
                            "S30",
                            format!(
                                "this argument of `{}` is already an aggregate",
                                name.to_ascii_uppercase()
                            ),
                            a,
                        );
                    }
                    widest = widest.max(s);
                }
                if aggregate { Shape::Agg } else { widest }
            }
        }
    }

    fn infer_case(&mut self, whens: &[(Expr, Expr)], els: Option<&Expr>) -> Ty {
        for (cond, _) in whens {
            self.require(cond, &[Ty::Bool], "a `CASE` condition");
        }
        // The result type is the branches' common type, or `Any` if they differ.
        // One unknown branch makes the whole result unknown, so the S23 lands
        // where the `CASE` is used rather than on an arbitrary branch.
        let mut result: Option<Ty> = None;
        let mut unknown = false;
        let branches = whens.iter().map(|(_, r)| r).chain(els);
        for r in branches {
            let t = self.infer(r);
            if t == Ty::Unknown {
                unknown = true;
                continue;
            }
            result = Some(match result {
                None => t,
                Some(prev) if prev == t || t == Ty::Any => prev,
                Some(Ty::Any) => t,
                Some(_) => Ty::Any,
            });
        }
        if unknown {
            return Ty::Unknown;
        }
        result.unwrap_or(Ty::Any)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> AssertExpr {
        AssertExpr::parse(s)
            .unwrap_or_else(|e| panic!("parse({s:?}) failed: {} at {}", e.message, e.at))
    }

    struct TestEnv;
    impl TestEnv {
        const COLUMNS: &[(&str, ColumnKind)] = &[
            ("n", ColumnKind::Number),
            ("qty", ColumnKind::Number),
            ("s", ColumnKind::String),
            ("postcode", ColumnKind::String),
            ("flag", ColumnKind::Bool),
            ("q3", ColumnKind::Bool),
            ("q4", ColumnKind::Bool),
            ("d", ColumnKind::Date),
            ("start_date", ColumnKind::Date),
            ("end_date", ColumnKind::Date),
            ("ts", ColumnKind::Datetime),
            ("u", ColumnKind::Untyped),
            ("addr", ColumnKind::Struct),
            ("tags", ColumnKind::List),
        ];
        /// Dotted field paths on the struct columns above; `addr.geo` nests.
        const FIELDS: &[(&str, ColumnKind)] = &[
            ("addr.zip", ColumnKind::String),
            ("addr.geo", ColumnKind::Struct),
            ("addr.geo.lat", ColumnKind::Number),
            ("addr.nick names", ColumnKind::String),
            ("addr.untyped", ColumnKind::Untyped),
        ];
    }
    impl CheckEnv for TestEnv {
        fn column(&self, name: &str) -> Option<ColumnKind> {
            Self::COLUMNS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, k)| *k)
        }
        fn field(&self, path: &[String]) -> Option<ColumnKind> {
            let joined = path.join(".");
            Self::FIELDS
                .iter()
                .find(|(p, _)| *p == joined)
                .map(|(_, k)| *k)
        }
        fn columns(&self) -> Vec<(String, ColumnKind)> {
            Self::COLUMNS
                .iter()
                .map(|(n, k)| (n.to_string(), *k))
                .collect()
        }
        fn is_date(&self, s: &str) -> bool {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
        }
        fn is_datetime(&self, s: &str) -> bool {
            chrono::DateTime::parse_from_rfc3339(s).is_ok()
        }
    }

    fn check_str(s: &str) -> Vec<Finding> {
        check(&parse(s), &TestEnv)
    }

    // --- parsing ---

    #[test]
    fn simple_comparison() {
        let e = parse("qty > 0");
        assert!(matches!(
            e.root.kind,
            ExprKind::Compare { op: CmpOp::Gt, .. }
        ));
    }

    #[test]
    fn precedence_or_binds_loosest() {
        // a AND b OR c  =>  (a AND b) OR c
        let e = parse("q3 AND q4 OR flag");
        assert!(matches!(e.root.kind, ExprKind::Or(..)));
    }

    #[test]
    fn precedence_arithmetic_tighter_than_comparison() {
        let e = parse("n + 1 <= 10");
        let ExprKind::Compare { lhs, .. } = &e.root.kind else {
            panic!("expected comparison at the root");
        };
        assert!(matches!(
            lhs.kind,
            ExprKind::Arith {
                op: ArithOp::Add,
                ..
            }
        ));
    }

    #[test]
    fn not_applies_to_comparison() {
        // NOT a = b  =>  NOT (a = b)
        let e = parse("NOT qty = 0");
        assert!(matches!(e.root.kind, ExprKind::Not(_)));
    }

    #[test]
    fn not_paren_form() {
        let e = parse("NOT(q3)");
        let ExprKind::Not(inner) = &e.root.kind else {
            panic!("expected NOT");
        };
        assert!(matches!(inner.kind, ExprKind::Column(_)));
    }

    #[test]
    fn between_and_not_stolen_by_top_level_and() {
        // The AND inside BETWEEN must not terminate the predicate early.
        let e = parse("n BETWEEN 1 AND 10 AND flag");
        assert!(matches!(e.root.kind, ExprKind::And(..)));
        let ExprKind::And(l, _) = &e.root.kind else {
            unreachable!()
        };
        assert!(matches!(l.kind, ExprKind::Between { .. }));
    }

    #[test]
    fn is_null_and_is_not_null() {
        assert!(matches!(
            parse("s IS NULL").root.kind,
            ExprKind::IsNull { negated: false, .. }
        ));
        assert!(matches!(
            parse("s IS NOT NULL").root.kind,
            ExprKind::IsNull { negated: true, .. }
        ));
    }

    #[test]
    fn in_list_and_not_in() {
        assert!(matches!(
            parse("n IN (1, 2, 3)").root.kind,
            ExprKind::In { negated: false, .. }
        ));
        assert!(matches!(
            parse("n NOT IN (1, 2)").root.kind,
            ExprKind::In { negated: true, .. }
        ));
    }

    #[test]
    fn like_and_similar_to() {
        assert!(matches!(
            parse("s LIKE 'a%'").root.kind,
            ExprKind::Like { negated: false, .. }
        ));
        assert!(matches!(
            parse("s NOT SIMILAR TO 'a.*'").root.kind,
            ExprKind::SimilarTo { negated: true, .. }
        ));
    }

    #[test]
    fn string_with_doubled_quote() {
        let e = parse("s = 'O''Brien'");
        let ExprKind::Compare { rhs, .. } = &e.root.kind else {
            panic!()
        };
        assert!(matches!(&rhs.kind, ExprKind::Str(v) if v == "O'Brien"));
    }

    #[test]
    fn functions_now_and_interval() {
        let e = parse("d >= NOW() - interval(2, weeks)");
        let ExprKind::Compare { rhs, .. } = &e.root.kind else {
            panic!()
        };
        let ExprKind::Arith { lhs, rhs, .. } = &rhs.kind else {
            panic!("expected arithmetic")
        };
        assert!(matches!(lhs.kind, ExprKind::Now));
        assert!(matches!(rhs.kind, ExprKind::Interval { .. }));
    }

    #[test]
    fn case_expression() {
        let e = parse("CASE WHEN q3 THEN qty ELSE 0 END > 5");
        assert!(matches!(e.root.kind, ExprKind::Compare { .. }));
    }

    #[test]
    fn columns_forms() {
        assert!(matches!(
            parse("COLUMNS(*) IS NOT NULL").root.kind,
            ExprKind::IsNull { .. }
        ));
        parse("COLUMNS('q[4-8]') IS NOT NULL");
        parse("COLUMNS([a, b, c]) IS NOT NULL");
    }

    #[test]
    fn quoted_column_names() {
        let e = parse("`creation date` IS NOT NULL");
        let ExprKind::IsNull { operand, .. } = &e.root.kind else {
            panic!()
        };
        assert!(matches!(&operand.kind, ExprKind::Column(c) if c[..] == ["creation date"]));
    }

    #[test]
    fn quoted_column_name_may_be_a_reserved_word() {
        let e = parse("`end` >= `start`");
        let ExprKind::Compare { lhs, rhs, .. } = &e.root.kind else {
            panic!()
        };
        assert!(matches!(&lhs.kind, ExprKind::Column(c) if c[..] == ["end"]));
        assert!(matches!(&rhs.kind, ExprKind::Column(c) if c[..] == ["start"]));
    }

    #[test]
    fn quoted_column_name_holds_any_character() {
        for (src, name) in [
            ("`a``b` IS NULL", "a`b"),
            ("`a.b` IS NULL", "a.b"),
            ("`café` IS NULL", "café"),
            ("`LENGTH(x)` IS NULL", "LENGTH(x)"),
        ] {
            let e = parse(src);
            let ExprKind::IsNull { operand, .. } = &e.root.kind else {
                panic!("{src}")
            };
            assert!(
                matches!(&operand.kind, ExprKind::Column(c) if c[..] == [name]),
                "{src}"
            );
        }
    }

    #[test]
    fn quoted_column_span_covers_the_backticks() {
        let src = "`a b` IS NULL";
        let e = parse(src);
        let ExprKind::IsNull { operand, .. } = &e.root.kind else {
            panic!()
        };
        assert_eq!(&src[operand.start..operand.end], "`a b`");
    }

    #[test]
    fn quoted_column_in_columns_list() {
        let e = parse("COLUMNS([`creation date`, b]) IS NOT NULL");
        let ExprKind::IsNull { operand, .. } = &e.root.kind else {
            panic!()
        };
        let ExprKind::Columns(ColumnsSelector::List(names)) = &operand.kind else {
            panic!("expected a list selector")
        };
        assert_eq!(
            names.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["creation date", "b"]
        );
    }

    #[test]
    fn rejects_unterminated_quoted_name() {
        let err = AssertExpr::parse("`a b IS NULL").unwrap_err();
        assert!(err.message.contains("unterminated"));
        assert_eq!(err.at, 0);
    }

    #[test]
    fn rejects_empty_quoted_name() {
        let err = AssertExpr::parse("`` IS NULL").unwrap_err();
        assert!(err.message.contains("empty"));
    }

    #[test]
    fn keyword_case_insensitive() {
        parse("qty > 0 and s is not null");
    }

    #[test]
    fn interval_word_boundary() {
        // A column named after a keyword prefix is still a column.
        let e = parse("intervals > 0");
        let ExprKind::Compare { lhs, .. } = &e.root.kind else {
            panic!()
        };
        assert!(matches!(&lhs.kind, ExprKind::Column(c) if c[..] == ["intervals"]));
    }

    #[test]
    fn spans_point_at_tokens() {
        let s = "qty > 0";
        let e = AssertExpr::parse(s).unwrap();
        let ExprKind::Compare { lhs, rhs, .. } = &e.root.kind else {
            panic!()
        };
        assert_eq!(&s[lhs.start..lhs.end], "qty");
        assert_eq!(&s[rhs.start..rhs.end], "0");
    }

    #[test]
    fn rejects_empty() {
        assert!(AssertExpr::parse("").is_err());
    }

    #[test]
    fn rejects_trailing_input() {
        assert!(AssertExpr::parse("qty > 0 garbage").is_err());
    }

    #[test]
    fn rejects_unterminated_string() {
        let err = AssertExpr::parse("s = 'abc").unwrap_err();
        assert!(err.message.contains("unterminated"));
    }

    #[test]
    fn rejects_bare_keyword_as_operand() {
        assert!(AssertExpr::parse("qty > AND").is_err());
    }

    // --- checking ---

    #[test]
    fn clean_expressions_have_no_findings() {
        assert!(check_str("qty > 0").is_empty());
        assert!(check_str("LENGTH(postcode) <= 10").is_empty());
        assert!(check_str("end_date >= start_date").is_empty());
        assert!(check_str("COLUMNS(*) IS NOT NULL").is_empty());
        assert!(check_str("NOT(q3) OR q4 IS NOT NULL").is_empty());
        assert!(check_str("d >= NOW() - interval(2, weeks)").is_empty());
        assert!(check_str("d >= '2000-01-01'").is_empty());
    }

    #[test]
    fn unknown_column_is_s20() {
        let f = check_str("missing > 0");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].code, "S20");
    }

    #[test]
    fn non_boolean_top_level_is_s21() {
        let f = check_str("qty");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].code, "S21");
        assert!(f[0].message.contains("boolean"));
    }

    #[test]
    fn type_mismatch_in_function_is_s21() {
        let f = check_str("LENGTH(qty) <= 10");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].code, "S21");
    }

    #[test]
    fn comparing_incompatible_types_is_s21() {
        let f = check_str("qty = s");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].code, "S21");
    }

    #[test]
    fn wrong_arity_is_s21() {
        let f = check_str("ROUND(qty, 2, 3) > 0");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("argument"))
        );
    }

    #[test]
    fn unknown_function_is_s21() {
        let f = check_str("SQRT(qty) > 0");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("unknown function"))
        );
    }

    #[test]
    fn two_columns_expressions_is_s21() {
        let f = check_str("COLUMNS(*) IS NOT NULL AND COLUMNS('x') IS NOT NULL");
        assert!(f.iter().any(|f| f.message.contains("at most one")));
    }

    #[test]
    fn bad_regex_is_s21() {
        let f = check_str("s SIMILAR TO '('");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("regular expression"))
        );
    }

    #[test]
    fn columns_list_unknown_name_is_s20() {
        let f = check_str("COLUMNS([qty, nope]) IS NOT NULL");
        assert!(
            f.iter()
                .any(|f| f.code == "S20" && f.message.contains("nope"))
        );
    }

    #[test]
    fn bad_interval_unit_is_s21() {
        let f = check_str("d >= NOW() - interval(2, fortnights)");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("interval unit"))
        );
    }

    #[test]
    fn untyped_column_where_a_type_matters_is_s23() {
        for expr in ["u > 5", "u", "LENGTH(u)", "u AND flag", "u + 1 > 0"] {
            let f = check_str(expr);
            assert!(
                f.iter()
                    .any(|f| f.code == "S23" && f.message.contains("`u`")),
                "expected S23 for `{expr}`, got {f:?}"
            );
        }
    }

    #[test]
    fn untyped_column_is_fine_where_no_type_is_needed() {
        assert!(check_str("u IS NOT NULL").is_empty());
        assert!(check_str("NOT(q3) OR u IS NULL").is_empty());
    }

    #[test]
    fn untyped_column_is_reported_once() {
        let f = check_str("u + 1 > 0");
        assert_eq!(f.iter().filter(|f| f.code == "S23").count(), 1);
    }

    #[test]
    fn columns_star_reports_untyped_columns_only_when_typed() {
        // `IS NOT NULL` asks nothing of `u`, but a boolean assertion does.
        assert!(check_str("COLUMNS(*) IS NOT NULL").is_empty());
        let f = check_str("COLUMNS(*)");
        assert!(
            f.iter()
                .any(|f| f.code == "S23" && f.message.contains("`u`")),
            "got {f:?}"
        );
    }

    #[test]
    fn columns_regex_matching_nothing_is_s22_warning() {
        let f = check_str("COLUMNS('zzz_nope') IS NOT NULL");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].code, "S22");
        assert_eq!(f[0].severity, FindingSeverity::Warning);
    }

    #[test]
    fn columns_regex_matching_something_is_clean() {
        // `q3`, `q4`, `qty` all contain `q`.
        assert!(check_str("COLUMNS('q') IS NOT NULL").is_empty());
    }

    #[test]
    fn columns_star_is_never_a_zero_match_warning() {
        assert!(check_str("COLUMNS(*) IS NOT NULL").is_empty());
    }

    #[test]
    fn columns_type_checked_against_matched_columns() {
        // `q3` and `q4` are booleans, so requiring them to be strings is S21.
        let f = check_str("LENGTH(COLUMNS('q[34]')) > 0");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("`q3`"))
        );
    }

    #[test]
    fn columns_type_ok_when_all_matches_fit() {
        // `start_date`/`end_date` are dates; comparing the selection to a date
        // literal is fine.
        assert!(check_str("COLUMNS('_date') >= '2000-01-01'").is_empty());
    }

    #[test]
    fn columns_comparison_against_wrong_type_is_s21() {
        // The `_date` columns are dates, not numbers.
        let f = check_str("COLUMNS('_date') > 0");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("start_date"))
        );
    }

    #[test]
    fn bare_columns_must_be_boolean_per_column() {
        // A bare COLUMNS selection of number columns is not a boolean assertion.
        let f = check_str("COLUMNS('qty')");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("qty"))
        );
        // A bare COLUMNS of booleans is a fine assertion.
        assert!(check_str("COLUMNS([q3, q4])").is_empty());
    }

    // --- struct fields (dot access) ---

    #[test]
    fn field_access_parses_to_path() {
        let e = parse("addr.geo.lat > 0");
        let ExprKind::Compare { lhs, .. } = &e.root.kind else {
            panic!("expected comparison at the root");
        };
        assert!(matches!(&lhs.kind, ExprKind::Column(c) if c[..] == ["addr", "geo", "lat"]));
    }

    #[test]
    fn field_segments_quote_independently() {
        let e = parse("LENGTH(`addr`.`nick names`) > 0 AND LENGTH(addr.`nick names`) > 0");
        assert!(check(&e, &TestEnv).is_empty());
    }

    #[test]
    fn dot_needs_a_field_name() {
        let err = AssertExpr::parse("addr. > 0").unwrap_err();
        assert!(err.message.contains("field name"));
        let err = AssertExpr::parse("addr.end IS NULL").unwrap_err();
        assert!(err.message.contains("keyword"));
    }

    #[test]
    fn field_access_typechecks_as_field_type() {
        assert!(check_str("LENGTH(addr.zip) <= 10").is_empty());
        assert!(check_str("addr.geo.lat BETWEEN -90 AND 90").is_empty());
        let f = check_str("addr.zip > 0");
        assert!(f.iter().any(|f| f.code == "S21"));
    }

    #[test]
    fn unknown_field_is_s20() {
        let f = check_str("addr.zpi IS NOT NULL");
        assert!(
            f.iter()
                .any(|f| f.code == "S20" && f.message.contains("no field `zpi`"))
        );
    }

    #[test]
    fn field_access_through_non_struct_is_s21() {
        let f = check_str("postcode.x = 'a'");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("not a struct"))
        );
        let f = check_str("tags.x = 'a'");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("list"))
        );
    }

    #[test]
    fn untyped_field_is_s23_where_type_matters() {
        assert!(check_str("addr.untyped IS NOT NULL").is_empty());
        let f = check_str("LENGTH(addr.untyped) > 0");
        assert!(
            f.iter()
                .any(|f| f.code == "S23" && f.message.contains("addr.untyped"))
        );
    }

    #[test]
    fn bare_struct_and_list_take_only_is_null() {
        assert!(check_str("addr IS NOT NULL").is_empty());
        assert!(check_str("tags IS NULL OR flag").is_empty());
        let f = check_str("addr = addr");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("struct"))
        );
        let f = check_str("tags");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("a list, not a boolean"))
        );
    }

    // --- aggregates ---

    #[test]
    fn aggregate_expressions_are_clean() {
        for s in [
            "COUNT(postcode) >= 0.9 * ROW_COUNT()",
            "COUNT_DISTINCT(s) <= 16",
            "AVG(n) BETWEEN 0 AND 100",
            "SUM(qty) > 0",
            "MIN(d) >= '2000-01-01'",
            "MAX(ts) <= NOW()",
            "ANY(flag)",
            "ALL(q3 OR q4)",
            "ROW_COUNT() > 0",
        ] {
            assert!(check_str(s).is_empty(), "{s}: {:?}", check_str(s));
        }
    }

    #[test]
    fn aggregate_names_are_case_insensitive() {
        assert!(check_str("min(n) <= max(n)").is_empty());
    }

    #[test]
    fn mixing_row_and_aggregate_grains_is_allowed() {
        assert!(check_str("qty <= 2 * MIN(qty)").is_empty());
        assert!(check_str("n <= MAX(n)").is_empty());
    }

    #[test]
    fn nested_aggregate_is_s30() {
        let src = "AVG(MIN(n)) > 0";
        let f = check_str(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].code, "S30");
        assert!(f[0].message.contains("`AVG`"), "{:?}", f[0].message);
        // The span points at the offending argument, not the whole call.
        assert_eq!(&src[f[0].start..f[0].end], "MIN(n)");
    }

    #[test]
    fn aggregate_nested_below_an_operator_is_still_s30() {
        let src = "SUM(MAX(n) + 1) > 0";
        let f = check_str(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].code, "S30");
        assert_eq!(&src[f[0].start..f[0].end], "MAX(n) + 1");
    }

    #[test]
    fn an_aggregate_of_a_scalar_function_is_fine() {
        assert!(check_str("MAX(LENGTH(postcode)) <= 10").is_empty());
    }

    #[test]
    fn a_scalar_function_of_an_aggregate_is_fine() {
        assert!(check_str("ABS(AVG(n)) < 1").is_empty());
    }

    #[test]
    fn min_max_return_their_argument_type() {
        // A date argument makes `MIN` a date, so a date literal compares.
        assert!(check_str("MIN(d) >= '2000-01-01'").is_empty());
        // A string argument makes it a string, which a bare number can't match.
        let f = check_str("MIN(s) >= 3");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("cannot compare")),
            "{f:?}"
        );
    }

    #[test]
    fn min_of_an_unordered_type_is_s21() {
        let f = check_str("MIN(flag)");
        assert!(
            f.iter().any(|f| f.code == "S21"
                && f.message
                    .contains("`MIN` expects a number, a string, a date, or a datetime")),
            "{f:?}"
        );
    }

    #[test]
    fn sum_and_avg_reject_non_numbers() {
        for s in ["SUM(s) > 0", "AVG(d) > 0"] {
            let f = check_str(s);
            assert!(
                f.iter()
                    .any(|f| f.code == "S21" && f.message.contains("expects a number")),
                "{s}: {f:?}"
            );
        }
    }

    #[test]
    fn any_and_all_require_a_boolean() {
        let f = check_str("ANY(n)");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("expects a boolean")),
            "{f:?}"
        );
    }

    #[test]
    fn row_count_takes_no_arguments() {
        let f = check_str("ROW_COUNT(n) > 0");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("takes 0 argument(s), found 1")),
            "{f:?}"
        );
    }

    #[test]
    fn count_accepts_anything_including_untyped_and_composite() {
        // `COUNT` asks only whether a value is null, so it never consults a type.
        for s in [
            "COUNT(u) >= 1",
            "COUNT(addr) >= 1",
            "COUNT(tags) >= 1",
            "COUNT(n) = ROW_COUNT()",
        ] {
            assert!(check_str(s).is_empty(), "{s}: {:?}", check_str(s));
        }
    }

    #[test]
    fn count_distinct_still_needs_a_declared_ordered_type() {
        let f = check_str("COUNT_DISTINCT(u) <= 4");
        assert!(f.iter().any(|f| f.code == "S23"), "{f:?}");
        let f = check_str("COUNT_DISTINCT(flag) <= 4");
        assert!(f.iter().any(|f| f.code == "S21"), "{f:?}");
    }

    #[test]
    fn a_bare_aggregate_must_still_be_boolean() {
        let f = check_str("SUM(n)");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("not a boolean")),
            "{f:?}"
        );
        assert!(check_str("ANY(flag)").is_empty());
    }

    #[test]
    fn an_aggregate_over_columns_distributes_over_the_selection() {
        // Every selected column must fit the aggregate's class.
        assert!(check_str("MAX(COLUMNS([n, qty])) > 0").is_empty());
        let f = check_str("SUM(COLUMNS('q[34]')) > 0");
        assert!(
            f.iter()
                .any(|f| f.code == "S21" && f.message.contains("column `q3` is a boolean")),
            "{f:?}"
        );
    }
}
