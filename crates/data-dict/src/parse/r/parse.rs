//! R source to [`RExpr`]: a scanner and a precedence-climbing parser.
//!
//! One expression, not a script — no assignment, no `if`, no braces. The
//! grammar covers what the [`R` targets](crate::emit) emit, plus the spellings
//! an author would naturally write for the same thing.

use super::ast::{PREC_NEG, PREC_NOT, RArg, RBinop, RExpr, RKind, RUnop};
use crate::assert_expr::ParseError;

pub fn parse(source: &str) -> Result<RExpr, ParseError> {
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

    // --- expressions ---

    /// Precedence climbing: parse a prefix, then absorb every infix operator
    /// binding at least as tightly as `min`.
    fn expr(&mut self, min: u8) -> Result<RExpr, ParseError> {
        let mut lhs = self.prefix()?;
        loop {
            self.skip_ws();
            let Some((op, len)) = self.peek_binop()? else {
                break;
            };
            let prec = op.prec();
            if prec < min {
                break;
            }
            self.pos += len;
            // Every one of these is left-associative, so the right operand must
            // bind strictly tighter to stay on the right.
            let rhs = self.expr(prec + 1)?;
            let span = lhs.union(&rhs);
            lhs = RExpr::new(RKind::Binary(op, Box::new(lhs), Box::new(rhs)), span);
        }
        Ok(lhs)
    }

    /// The infix operator at the cursor, and how many bytes it takes.
    fn peek_binop(&mut self) -> Result<Option<(RBinop, usize)>, ParseError> {
        let rest = &self.text[self.pos..];
        // Longest match first, so `<=` doesn't read as `<`.
        for (symbol, op) in [
            ("==", RBinop::Eq),
            ("!=", RBinop::Ne),
            ("<=", RBinop::Le),
            (">=", RBinop::Ge),
            ("%%", RBinop::Mod),
            ("%in%", RBinop::In),
        ] {
            if rest.starts_with(symbol) {
                return Ok(Some((op, symbol.len())));
            }
        }
        // `&&` and `||` are the scalar forms, which don't vectorise; refusing
        // is better than silently reading them as the vector ones.
        if rest.starts_with("&&") || rest.starts_with("||") {
            let which = &rest[..2];
            return Err(self.err(format!(
                "`{which}` cannot be translated: it is R's scalar operator, which tests only \
                 the first element; use `{}`",
                &which[..1]
            )));
        }
        // Before the single-character operators, or `qty <- 1` would read as
        // `qty < -1`, which is a change of meaning rather than an error. R
        // tokenizes the arrow greedily and so does this.
        if rest.starts_with("<-") || rest.starts_with("->") || rest.starts_with("<<-") {
            return Err(self.err("assignment cannot be translated: an expression is not a script"));
        }
        if rest.starts_with("%/%") {
            return Err(self.err(
                "`%/%` cannot be translated: the language has no integer division; \
                 use `FLOOR(x / y)`",
            ));
        }
        // An infix operator this reader doesn't know, rather than a stray `%`.
        if rest.starts_with('%')
            && let Some(end) = rest[1..].find('%')
        {
            let name = &rest[..end + 2];
            return Err(self.err(format!(
                "`{name}` cannot be translated: the language has no such operator"
            )));
        }
        for (symbol, op) in [
            ("<", RBinop::Lt),
            (">", RBinop::Gt),
            ("+", RBinop::Add),
            ("-", RBinop::Sub),
            ("*", RBinop::Mul),
            ("/", RBinop::Div),
            ("&", RBinop::And),
            ("|", RBinop::Or),
            ("~", RBinop::Formula),
        ] {
            if rest.starts_with(symbol) {
                return Ok(Some((op, symbol.len())));
            }
        }
        if rest.starts_with('^') {
            return Err(
                self.err("`^` cannot be translated: the language has no exponentiation operator")
            );
        }
        Ok(None)
    }

    fn prefix(&mut self) -> Result<RExpr, ParseError> {
        self.skip_ws();
        let start = self.pos;
        if self.eat("!") {
            let operand = self.expr(PREC_NOT)?;
            let span = (start, operand.span.1);
            return Ok(RExpr::new(
                RKind::Unary(RUnop::Not, Box::new(operand)),
                span,
            ));
        }
        if self.peek() == Some(b'-') {
            self.pos += 1;
            let operand = self.expr(PREC_NEG)?;
            let span = (start, operand.span.1);
            return Ok(RExpr::new(
                RKind::Unary(RUnop::Neg, Box::new(operand)),
                span,
            ));
        }
        // Unary plus is a no-op in R, so it leaves no node.
        if self.peek() == Some(b'+') {
            self.pos += 1;
            return self.expr(PREC_NEG);
        }
        self.postfix()
    }

    /// A primary, then any `$field` and `[index]` that follow it.
    fn postfix(&mut self) -> Result<RExpr, ParseError> {
        let mut expr = self.primary()?;
        loop {
            self.skip_ws();
            if self.eat("$") {
                let (field, _) = self.name()?;
                let span = (expr.span.0, self.pos);
                expr = RExpr::new(RKind::Dollar(Box::new(expr), field), span);
            } else if self.text[self.pos..].starts_with("[[") {
                return Err(
                    self.err("`[[` cannot be translated: the language reaches a field with a dot")
                );
            } else if self.eat("[") {
                let index = self.expr(0)?;
                self.expect("]")?;
                let span = (expr.span.0, self.pos);
                expr = RExpr::new(RKind::Index(Box::new(expr), Box::new(index)), span);
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn primary(&mut self) -> Result<RExpr, ParseError> {
        self.skip_ws();
        let start = self.pos;
        let Some(b) = self.peek() else {
            return Err(self.err("expected an expression"));
        };
        if b == b'(' {
            self.pos += 1;
            let inner = self.expr(0)?;
            self.expect(")")?;
            // Parentheses only group; the tree records the grouping already.
            return Ok(inner);
        }
        if b == b'"' || b == b'\'' {
            let text = self.string(b)?;
            return Ok(RExpr::new(RKind::Str(text), (start, self.pos)));
        }
        if b.is_ascii_digit() || (b == b'.' && self.at(1).is_some_and(|c| c.is_ascii_digit())) {
            return self.number();
        }
        // `\(x) body`
        if b == b'\\' {
            self.pos += 1;
            return self.lambda(start);
        }
        if b == b'`' || is_name_start(b) {
            let (name, name_end) = self.name()?;
            if name == "function" {
                return self.lambda(start);
            }
            self.skip_ws();
            if self.peek() == Some(b'(') {
                self.pos += 1;
                let args = self.args()?;
                return Ok(RExpr::new(
                    RKind::Call { fun: name, args },
                    (start, self.pos),
                ));
            }
            let span = (start, name_end);
            return Ok(RExpr::new(literal_or_name(&name), span));
        }
        Err(self.err("expected an expression"))
    }

    /// The parameter list and body of `\(x) …` or `function(x) …`, with the
    /// backslash or keyword already consumed.
    fn lambda(&mut self, start: usize) -> Result<RExpr, ParseError> {
        self.expect("(")?;
        let (param, _) = self.name()?;
        self.skip_ws();
        if self.peek() == Some(b',') {
            return Err(self.err(
                "a lambda of more than one argument cannot be translated: \
                 the language applies a predicate to one column at a time",
            ));
        }
        self.expect(")")?;
        let body = self.expr(0)?;
        let span = (start, body.span.1);
        Ok(RExpr::new(
            RKind::Lambda {
                param,
                body: Box::new(body),
            },
            span,
        ))
    }

    /// A call's arguments, with the `(` already consumed.
    fn args(&mut self) -> Result<Vec<RArg>, ParseError> {
        let mut args = Vec::new();
        self.skip_ws();
        if self.eat(")") {
            return Ok(args);
        }
        loop {
            self.skip_ws();
            // A named argument is `name = value`, distinguished from `==`.
            let name = self.argument_name();
            let value = self.expr(0)?;
            args.push(RArg { name, value });
            self.skip_ws();
            if self.eat(",") {
                continue;
            }
            self.expect(")")?;
            break;
        }
        Ok(args)
    }

    /// The `name =` of a named argument, when that is what is next.
    fn argument_name(&mut self) -> Option<String> {
        let save = self.pos;
        if !self.peek().is_some_and(is_name_start) {
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

    /// An R name: bare, or backtick-quoted for one that isn't syntactic. A `.`
    /// is part of a bare name, so `is.na` is one name.
    fn name(&mut self) -> Result<(String, usize), ParseError> {
        self.skip_ws();
        if self.peek() == Some(b'`') {
            let start = self.pos;
            self.pos += 1;
            let mut out = String::new();
            loop {
                match self.peek() {
                    None => return Err(self.err_at(start, "unterminated backtick name")),
                    Some(b'`') => {
                        self.pos += 1;
                        break;
                    }
                    Some(b'\\') if self.at(1) == Some(b'`') => {
                        out.push('`');
                        self.pos += 2;
                    }
                    _ => {
                        let from = self.pos;
                        crate::expr_lex::advance_char(self.src, &mut self.pos);
                        out.push_str(&self.text[from..self.pos]);
                    }
                }
            }
            return Ok((out, self.pos));
        }
        let start = self.pos;
        if !self.peek().is_some_and(is_name_start) {
            return Err(self.err("expected a name"));
        }
        self.pos += 1;
        while self.peek().is_some_and(is_name_continue) {
            self.pos += 1;
        }
        Ok((self.text[start..self.pos].to_string(), self.pos))
    }

    fn number(&mut self) -> Result<RExpr, ParseError> {
        let start = self.pos;
        if self.text[self.pos..].starts_with("0x") || self.text[self.pos..].starts_with("0X") {
            return Err(self.err("a hexadecimal literal cannot be translated"));
        }
        let mut float = false;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek() == Some(b'.') {
            float = true;
            self.pos += 1;
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
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
        let text = &self.text[start..self.pos];
        let value: f64 = text
            .parse()
            .map_err(|_| self.err_at(start, "malformed number"))?;
        // An `L` suffix says integer outright; without one, a literal with no
        // point and no exponent is still an integer to the language.
        let integer = if self.peek() == Some(b'L') {
            self.pos += 1;
            true
        } else {
            !float
        };
        Ok(RExpr::new(RKind::Num { value, integer }, (start, self.pos)))
    }

    /// A quoted string. R escapes with a backslash, unlike the language's own
    /// doubled quote.
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
                        b'`' => '`',
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

/// R's reserved constants, which are names syntactically but values here.
fn literal_or_name(name: &str) -> RKind {
    match name {
        "TRUE" | "T" => RKind::Logical(true),
        "FALSE" | "F" => RKind::Logical(false),
        // The typed `NA`s differ only in the type R gives them, which the
        // language decides for itself.
        "NA" | "NA_integer_" | "NA_real_" | "NA_character_" => RKind::Na,
        "Inf" => RKind::Inf,
        "NaN" => RKind::NaN,
        "NULL" => RKind::Null,
        other => RKind::Name(other.to_string()),
    }
}

fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'.' || b == b'_'
}

fn is_name_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'_'
}
