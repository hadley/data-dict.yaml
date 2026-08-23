//! Python source to [`PyExpr`]: a scanner and a precedence-climbing parser.
//!
//! One expression, not a script — no statements, no lambdas, no comprehensions.
//! The grammar covers what the [`Python(polars)` target](crate::emit) emits,
//! plus the spellings an author would naturally write for the same thing.

use super::ast::{PREC_UNARY, PyArg, PyBinop, PyExpr, PyKind, PyUnop};
use crate::assert_expr::ParseError;
use crate::parse::untranslatable;

pub fn parse(source: &str) -> Result<PyExpr, ParseError> {
    let mut p = Parser {
        src: source.as_bytes(),
        text: source,
        pos: 0,
    };
    let expr = p.expr(0)?;
    p.skip_ws();
    if !p.is_eof() {
        return Err(p.err("unexpected trailing input"));
    }
    Ok(expr)
}

struct Parser<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn at(&self, offset: usize) -> Option<u8> {
        self.src.get(self.pos + offset).copied()
    }

    fn err(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            message: message.into(),
            at: self.pos,
        }
    }

    fn err_at(&self, at: usize, message: impl Into<String>) -> ParseError {
        ParseError {
            message: message.into(),
            at,
        }
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.pos += 1;
            } else if b == b'#' {
                while self.peek().is_some_and(|b| b != b'\n') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, symbol: &str) -> bool {
        self.skip_ws();
        if self.text[self.pos..].starts_with(symbol) {
            self.pos += symbol.len();
            return true;
        }
        false
    }

    fn expect(&mut self, symbol: &str) -> Result<(), ParseError> {
        if self.eat(symbol) {
            Ok(())
        } else {
            Err(self.err(format!("expected `{symbol}`")))
        }
    }

    fn expr(&mut self, min: u8) -> Result<PyExpr, ParseError> {
        let mut lhs = self.unary()?;
        let mut compared = false;
        loop {
            self.skip_ws();
            let Some((op, len)) = self.peek_binop()? else {
                break;
            };
            let prec = op.prec();
            if prec < min {
                break;
            }
            // Python chains comparisons — `lo < x < hi` means `lo < x and
            // x < hi` — and the language has no such form, so reading one as a
            // left-nested pair would change its meaning.
            if op.is_comparison() {
                if compared {
                    return Err(untranslatable(
                        "a chained comparison",
                        "the language compares two operands at a time; write `BETWEEN`, or \
                         two comparisons joined with `&`",
                        self.pos,
                    ));
                }
                compared = true;
            }
            self.pos += len;
            let rhs = self.expr(prec + 1)?;
            let span = lhs.union(&rhs);
            lhs = PyExpr::new(PyKind::Binary(op, Box::new(lhs), Box::new(rhs)), span);
        }
        Ok(lhs)
    }

    fn peek_binop(&mut self) -> Result<Option<(PyBinop, usize)>, ParseError> {
        let rest = &self.text[self.pos..];
        for (symbol, op) in [
            ("==", PyBinop::Eq),
            ("!=", PyBinop::Ne),
            ("<=", PyBinop::Le),
            (">=", PyBinop::Ge),
        ] {
            if rest.starts_with(symbol) {
                return Ok(Some((op, symbol.len())));
            }
        }
        // The short-circuiting operators work on Python truth values, not on
        // polars expressions, and reading them as `&`/`|` would be wrong.
        for word in ["and", "or", "not"] {
            if rest.starts_with(word)
                && !self
                    .src
                    .get(self.pos + word.len())
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                return Err(untranslatable(
                    format!("`{word}`"),
                    "polars expressions combine with `&`, `|` and `~`, which are the \
                     three-valued operators",
                    self.pos,
                ));
            }
        }
        if rest.starts_with("//") {
            return Err(untranslatable(
                "`//`",
                "the language has no integer division; use `FLOOR(x / y)`",
                self.pos,
            ));
        }
        if rest.starts_with("**") {
            return Err(untranslatable(
                "`**`",
                "the language has no exponentiation operator",
                self.pos,
            ));
        }
        for (symbol, op) in [
            ("<", PyBinop::Lt),
            (">", PyBinop::Gt),
            ("+", PyBinop::Add),
            ("-", PyBinop::Sub),
            ("*", PyBinop::Mul),
            ("/", PyBinop::Div),
            ("%", PyBinop::Mod),
            ("&", PyBinop::BitAnd),
            ("|", PyBinop::BitOr),
        ] {
            if rest.starts_with(symbol) {
                return Ok(Some((op, symbol.len())));
            }
        }
        Ok(None)
    }

    fn unary(&mut self) -> Result<PyExpr, ParseError> {
        self.skip_ws();
        let start = self.pos;
        for (symbol, op) in [("-", PyUnop::Neg), ("~", PyUnop::Invert)] {
            if self.peek() == Some(symbol.as_bytes()[0]) {
                self.pos += 1;
                let operand = self.expr(PREC_UNARY)?;
                let span = (start, operand.span.1);
                return Ok(PyExpr::new(PyKind::Unary(op, Box::new(operand)), span));
            }
        }
        if self.peek() == Some(b'+') {
            self.pos += 1;
            return self.expr(PREC_UNARY);
        }
        self.postfix()
    }

    /// A primary, then the `.attribute` and `(call)` that follow it — the
    /// method chains polars is written in.
    fn postfix(&mut self) -> Result<PyExpr, ParseError> {
        let mut expr = self.primary()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'.') && !self.at(1).is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
                let (name, _) = self.name()?;
                let span = (expr.span.0, self.pos);
                expr = PyExpr::new(PyKind::Attr(Box::new(expr), name), span);
            } else if self.eat("(") {
                let args = self.args()?;
                let span = (expr.span.0, self.pos);
                expr = PyExpr::new(
                    PyKind::Call {
                        callee: Box::new(expr),
                        args,
                    },
                    span,
                );
            } else if self.peek() == Some(b'[') {
                return Err(untranslatable(
                    "subscripting",
                    "the language has no indexing; a rule is written over whole columns",
                    self.pos,
                ));
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn primary(&mut self) -> Result<PyExpr, ParseError> {
        self.skip_ws();
        let start = self.pos;
        let Some(b) = self.peek() else {
            return Err(self.err("expected an expression"));
        };
        if b == b'(' {
            self.pos += 1;
            let inner = self.expr(0)?;
            self.expect(")")?;
            return Ok(inner);
        }
        if b == b'[' {
            self.pos += 1;
            let mut items = Vec::new();
            self.skip_ws();
            if !self.eat("]") {
                loop {
                    items.push(self.expr(0)?);
                    self.skip_ws();
                    if self.eat(",") {
                        self.skip_ws();
                        if self.eat("]") {
                            break;
                        }
                        continue;
                    }
                    self.expect("]")?;
                    break;
                }
            }
            return Ok(PyExpr::new(PyKind::List(items), (start, self.pos)));
        }
        if b == b'"' || b == b'\'' {
            let text = self.string(b)?;
            return Ok(PyExpr::new(PyKind::Str(text), (start, self.pos)));
        }
        if b.is_ascii_digit() || (b == b'.' && self.at(1).is_some_and(|c| c.is_ascii_digit())) {
            return self.number();
        }
        if b.is_ascii_alphabetic() || b == b'_' {
            let (name, name_end) = self.name()?;
            let kind = match name.as_str() {
                "True" => PyKind::Bool(true),
                "False" => PyKind::Bool(false),
                "None" => PyKind::None,
                other => PyKind::Name(other.to_string()),
            };
            return Ok(PyExpr::new(kind, (start, name_end)));
        }
        Err(self.err("expected an expression"))
    }

    /// A call's arguments, with the `(` already consumed.
    fn args(&mut self) -> Result<Vec<PyArg>, ParseError> {
        let mut args = Vec::new();
        self.skip_ws();
        if self.eat(")") {
            return Ok(args);
        }
        loop {
            self.skip_ws();
            let name = self.argument_name();
            let value = self.expr(0)?;
            args.push(PyArg { name, value });
            self.skip_ws();
            if self.eat(",") {
                self.skip_ws();
                if self.eat(")") {
                    break;
                }
                continue;
            }
            self.expect(")")?;
            break;
        }
        Ok(args)
    }

    /// The `name=` of a keyword argument, when that is what is next.
    fn argument_name(&mut self) -> Option<String> {
        let save = self.pos;
        if !self
            .peek()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        {
            return None;
        }
        let Ok((name, _)) = self.name() else {
            self.pos = save;
            return None;
        };
        self.skip_ws();
        // `=` but not `==`, which would make this an ordinary comparison.
        if self.peek() == Some(b'=') && self.at(1) != Some(b'=') {
            self.pos += 1;
            return Some(name);
        }
        self.pos = save;
        None
    }

    fn name(&mut self) -> Result<(String, usize), ParseError> {
        self.skip_ws();
        let start = self.pos;
        if !self
            .peek()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        {
            return Err(self.err("expected a name"));
        }
        self.pos += 1;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            self.pos += 1;
        }
        Ok((self.text[start..self.pos].to_string(), self.pos))
    }

    fn number(&mut self) -> Result<PyExpr, ParseError> {
        let start = self.pos;
        if self.text[self.pos..].starts_with("0x") || self.text[self.pos..].starts_with("0X") {
            return Err(untranslatable(
                "a hexadecimal literal",
                "the language writes numbers in base ten",
                start,
            ));
        }
        let mut float = false;
        while self.peek().is_some_and(|b| b.is_ascii_digit() || b == b'_') {
            self.pos += 1;
        }
        if self.peek() == Some(b'.') {
            float = true;
            self.pos += 1;
            while self.peek().is_some_and(|b| b.is_ascii_digit() || b == b'_') {
                self.pos += 1;
            }
        }
        if self.peek().is_some_and(|b| b == b'e' || b == b'E') {
            float = true;
            self.pos += 1;
            if self.peek().is_some_and(|b| b == b'+' || b == b'-') {
                self.pos += 1;
            }
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text = self.text[start..self.pos].replace('_', "");
        let kind = if float {
            PyKind::Float(
                text.parse()
                    .map_err(|_| self.err_at(start, "malformed number"))?,
            )
        } else {
            PyKind::Int(
                text.parse()
                    .map_err(|_| self.err_at(start, "malformed number"))?,
            )
        };
        Ok(PyExpr::new(kind, (start, self.pos)))
    }

    /// A quoted string. Python's prefixes (`r"…"`, `f"…"`) are not accepted:
    /// a raw string would change what the escapes mean, and an f-string is a
    /// computation the language has no equivalent of.
    fn string(&mut self, quote: u8) -> Result<String, ParseError> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err_at(start, "unterminated string")),
                Some(b) if b == quote => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let Some(escape) = self.peek() else {
                        return Err(self.err_at(start, "unterminated string"));
                    };
                    self.pos += 1;
                    out.push(match escape {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'0' => '\0',
                        b'\\' => '\\',
                        b'"' => '"',
                        b'\'' => '\'',
                        other => {
                            return Err(self.err_at(
                                self.pos - 2,
                                format!("unknown escape `\\{}`", other as char),
                            ));
                        }
                    });
                }
                _ => {
                    let from = self.pos;
                    crate::expr_lex::advance_char(self.src, &mut self.pos);
                    out.push_str(&self.text[from..self.pos]);
                }
            }
        }
    }
}
