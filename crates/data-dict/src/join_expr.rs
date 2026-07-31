//! Recursive-descent parser for the `join` expression mini-language used in
//! `relationships[*].join`.
//!
//! Grammar:
//!
//! ```text
//! join     := conjunct ("AND" conjunct)*
//! conjunct := qcol op qcol
//! qcol     := name "." name
//! op       := "=" | "==" | ">=" | "<=" | ">" | "<"
//! name     := IDENT | QUOTED
//! IDENT    := [A-Za-z_][A-Za-z0-9_]*
//! QUOTED   := "`" ( [^`] | "``" )+ "`"
//! ```
//!
//! `AND` is matched case-insensitively. Whitespace is permitted between
//! tokens. Each side of the `.` is quoted independently, so a name that isn't
//! an `IDENT` stays referenceable; a `.` inside backticks is part of the name. The parser tracks byte offsets within the input string so we can
//! point diagnostics at the failing token.

#[derive(Debug, Clone)]
pub struct JoinExpr {
    pub conjuncts: Vec<JoinConjunct>,
}

#[derive(Debug, Clone)]
pub struct JoinConjunct {
    pub lhs: QCol,
    pub op: JoinOp,
    pub rhs: QCol,
}

#[derive(Debug, Clone)]
pub struct QCol {
    pub table: String,
    pub column: String,
    /// Byte offset of the qualified column within the join string.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinOp {
    Eq,
    Ge,
    Le,
    Gt,
    Lt,
}

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    /// Byte offset of the failing token (or end-of-string) within the join
    /// expression.
    pub at: usize,
}

impl JoinExpr {
    pub fn parse(input: &str) -> Result<JoinExpr, ParseError> {
        let mut p = Parser::new(input);
        let mut conjuncts = vec![p.parse_conjunct()?];
        loop {
            p.skip_ws();
            if p.is_eof() {
                break;
            }
            p.expect_keyword("and")?;
            conjuncts.push(p.parse_conjunct()?);
        }
        Ok(JoinExpr { conjuncts })
    }

    /// Distinct table names referenced by any `qcol` in the expression.
    /// Order matches first-appearance order in the source so diagnostics are
    /// stable.
    pub fn tables(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for c in &self.conjuncts {
            for q in [&c.lhs, &c.rhs] {
                if !out.iter().any(|t| *t == q.table) {
                    out.push(&q.table);
                }
            }
        }
        out
    }

    /// All qualified column references, in source order.
    pub fn qcols(&self) -> impl Iterator<Item = &QCol> {
        self.conjuncts
            .iter()
            .flat_map(|c| [&c.lhs, &c.rhs].into_iter())
    }
}

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

    fn parse_conjunct(&mut self) -> Result<JoinConjunct, ParseError> {
        let lhs = self.parse_qcol()?;
        self.skip_ws();
        let op = self.parse_op()?;
        self.skip_ws();
        let rhs = self.parse_qcol()?;
        Ok(JoinConjunct { lhs, op, rhs })
    }

    fn parse_qcol(&mut self) -> Result<QCol, ParseError> {
        self.skip_ws();
        let start = self.pos;
        let table = self.parse_ident()?;
        if self.peek() != Some(b'.') {
            return Err(self.err("expected `.` after table name"));
        }
        self.pos += 1;
        let column = self.parse_ident()?;
        Ok(QCol {
            table,
            column,
            start,
            end: self.pos,
        })
    }

    fn parse_ident(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        match self.peek() {
            Some(b'`') => return self.parse_quoted_name(),
            Some(b) if b.is_ascii_alphabetic() || b == b'_' => self.pos += 1,
            _ => return Err(self.err("expected identifier")),
        }
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(std::str::from_utf8(&self.src[start..self.pos])
            .expect("identifier bytes are ASCII")
            .to_string())
    }

    /// Parse a backtick-quoted table or column name, leaving `self.pos` after
    /// the closing backtick. Errors point at the opening backtick, the token
    /// the reader has to fix.
    fn parse_quoted_name(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        debug_assert_eq!(self.peek(), Some(b'`'));
        self.pos += 1;
        let mut name = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(ParseError {
                        message: "unterminated quoted name".into(),
                        at: start,
                    });
                }
                Some(b'`') => {
                    // A doubled backtick is a literal backtick; a lone one ends it.
                    if self.src.get(self.pos + 1) == Some(&b'`') {
                        name.push('`');
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                        break;
                    }
                }
                Some(_) => {
                    let ch_start = self.pos;
                    self.advance_char();
                    name.push_str(
                        std::str::from_utf8(&self.src[ch_start..self.pos])
                            .expect("input is valid utf-8"),
                    );
                }
            }
        }
        if name.is_empty() {
            return Err(ParseError {
                message: "empty quoted name".into(),
                at: start,
            });
        }
        Ok(name)
    }

    /// Step over one whole UTF-8 code point.
    fn advance_char(&mut self) {
        self.pos += 1;
        while let Some(&b) = self.src.get(self.pos) {
            if b & 0xC0 == 0x80 {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_op(&mut self) -> Result<JoinOp, ParseError> {
        // Order matters: longer operators first.
        for (lit, op) in [
            (">=", JoinOp::Ge),
            ("<=", JoinOp::Le),
            ("==", JoinOp::Eq),
            ("=", JoinOp::Eq),
            (">", JoinOp::Gt),
            ("<", JoinOp::Lt),
        ] {
            if self.src[self.pos..].starts_with(lit.as_bytes()) {
                self.pos += lit.len();
                return Ok(op);
            }
        }
        Err(self.err("expected one of `=`, `>=`, `<=`, `>`, `<`"))
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), ParseError> {
        let end = self.pos + kw.len();
        if end > self.src.len() {
            return Err(self.err(format!("expected `{}`", kw.to_uppercase())));
        }
        let slice = &self.src[self.pos..end];
        if !slice.eq_ignore_ascii_case(kw.as_bytes()) {
            return Err(self.err(format!("expected `{}`", kw.to_uppercase())));
        }
        // Keyword must be followed by a non-identifier character (so we don't
        // match `andante` as `AND` + `ante`).
        if let Some(&b) = self.src.get(end)
            && (b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(self.err(format!("expected `{}`", kw.to_uppercase())));
        }
        self.pos = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> JoinExpr {
        JoinExpr::parse(s)
            .unwrap_or_else(|e| panic!("parse({s:?}) failed: {} at {}", e.message, e.at))
    }

    #[test]
    fn simple_equality() {
        let j = parse("food.fdc_id = food_nutrient.fdc_id");
        assert_eq!(j.conjuncts.len(), 1);
        assert_eq!(j.conjuncts[0].lhs.table, "food");
        assert_eq!(j.conjuncts[0].lhs.column, "fdc_id");
        assert_eq!(j.conjuncts[0].op, JoinOp::Eq);
        assert_eq!(j.conjuncts[0].rhs.table, "food_nutrient");
        assert_eq!(j.tables(), vec!["food", "food_nutrient"]);
    }

    #[test]
    fn self_join() {
        let j = parse("otters.pup_number = otters.otter_no");
        assert_eq!(j.tables(), vec!["otters"]);
    }

    #[test]
    fn multi_conjunct_with_and() {
        let j = parse("t1.date >= t2.start AND t1.date <= t2.end");
        assert_eq!(j.conjuncts.len(), 2);
        assert_eq!(j.conjuncts[0].op, JoinOp::Ge);
        assert_eq!(j.conjuncts[1].op, JoinOp::Le);
        assert_eq!(j.tables(), vec!["t1", "t2"]);
    }

    #[test]
    fn and_is_case_insensitive() {
        let j = parse("a.x = b.y and a.z = b.w");
        assert_eq!(j.conjuncts.len(), 2);
    }

    #[test]
    fn leading_and_trailing_whitespace() {
        let j = parse("  a.x = b.y  ");
        assert_eq!(j.conjuncts.len(), 1);
    }

    #[test]
    fn accepts_double_equals() {
        // `==` is an alternate spelling of `=`. Useful because most
        // programming languages spell equality that way and the validator
        // shouldn't punish that habit.
        let j = parse("food.fdc_id == food_nutrient.fdc_id");
        assert_eq!(j.conjuncts.len(), 1);
        assert_eq!(j.conjuncts[0].op, JoinOp::Eq);
    }

    #[test]
    fn quoted_names() {
        let j = parse("`other studies`.`creation date` = study.id");
        assert_eq!(j.conjuncts[0].lhs.table, "other studies");
        assert_eq!(j.conjuncts[0].lhs.column, "creation date");
        assert_eq!(j.conjuncts[0].rhs.table, "study");
    }

    #[test]
    fn quoted_name_holds_dots_and_backticks() {
        let j = parse("`a.b`.`c``d` = t.x");
        assert_eq!(j.conjuncts[0].lhs.table, "a.b");
        assert_eq!(j.conjuncts[0].lhs.column, "c`d");
    }

    #[test]
    fn quoted_name_spans_cover_the_backticks() {
        let j = parse("`a b`.c = t.x");
        let lhs = &j.conjuncts[0].lhs;
        assert_eq!(&"`a b`.c = t.x"[lhs.start..lhs.end], "`a b`.c");
    }

    #[test]
    fn rejects_unterminated_quoted_name() {
        let err = JoinExpr::parse("`a b.c = t.x").unwrap_err();
        assert!(err.message.contains("unterminated"));
        assert_eq!(err.at, 0);
    }

    #[test]
    fn rejects_empty_quoted_name() {
        let err = JoinExpr::parse("t.`` = t.x").unwrap_err();
        assert!(err.message.contains("empty"));
    }

    #[test]
    fn rejects_missing_dot() {
        let err = JoinExpr::parse("food = food_nutrient.fdc_id").unwrap_err();
        assert!(err.message.contains('.'));
    }

    #[test]
    fn rejects_unknown_operator() {
        let err = JoinExpr::parse("a.x ~ b.y").unwrap_err();
        assert!(err.message.contains('='));
    }

    #[test]
    fn rejects_trailing_and() {
        let err = JoinExpr::parse("a.x = b.y AND").unwrap_err();
        // After consuming `AND`, the parser tries to read a qcol.
        assert!(err.message.contains("identifier"));
    }

    #[test]
    fn rejects_andante_as_and() {
        // The bare keyword check must not greedily match identifier prefixes.
        let err = JoinExpr::parse("a.x = b.y andante c.z = d.w").unwrap_err();
        assert!(err.message.contains("AND"));
    }

    #[test]
    fn qcol_byte_spans_are_correct() {
        let s = "food.fdc_id = food_nutrient.fdc_id";
        let j = JoinExpr::parse(s).unwrap();
        let lhs = &j.conjuncts[0].lhs;
        assert_eq!(&s[lhs.start..lhs.end], "food.fdc_id");
        let rhs = &j.conjuncts[0].rhs;
        assert_eq!(&s[rhs.start..rhs.end], "food_nutrient.fdc_id");
    }

    #[test]
    fn all_operators_parse() {
        let cases = [
            ("a.x = b.y", JoinOp::Eq),
            ("a.x == b.y", JoinOp::Eq),
            ("a.x >= b.y", JoinOp::Ge),
            ("a.x <= b.y", JoinOp::Le),
            ("a.x > b.y", JoinOp::Gt),
            ("a.x < b.y", JoinOp::Lt),
        ];
        for (s, op) in cases {
            let j = parse(s);
            assert_eq!(j.conjuncts[0].op, op, "operator mismatch for {s:?}");
        }
    }

    #[test]
    fn two_char_operators_are_not_split() {
        // `>=` must win over `>`, otherwise the trailing `=` would be left for
        // the rhs qcol parse and the operator would be wrong.
        assert_eq!(parse("a.x >= b.y").conjuncts[0].op, JoinOp::Ge);
        assert_eq!(parse("a.x <= b.y").conjuncts[0].op, JoinOp::Le);
    }

    #[test]
    fn whitespace_around_operator_is_optional() {
        let j = parse("a.x=b.y");
        assert_eq!(j.conjuncts.len(), 1);
        assert_eq!(j.conjuncts[0].lhs.column, "x");
        assert_eq!(j.conjuncts[0].op, JoinOp::Eq);
        assert_eq!(j.conjuncts[0].rhs.column, "y");
    }

    #[test]
    fn three_conjuncts() {
        let j = parse("a.x = b.y AND c.z = d.w AND e.p = f.q");
        assert_eq!(j.conjuncts.len(), 3);
        assert_eq!(j.tables(), vec!["a", "b", "c", "d", "e", "f"]);
    }

    #[test]
    fn tables_dedup_in_first_appearance_order() {
        // `b` and `a` reappear in the second conjunct but must not be repeated.
        let j = parse("a.x = b.y AND b.z = a.w");
        assert_eq!(j.tables(), vec!["a", "b"]);
    }

    #[test]
    fn qcols_yields_all_refs_in_source_order() {
        let j = parse("a.x = b.y AND c.z = d.w");
        let cols: Vec<(&str, &str)> = j
            .qcols()
            .map(|q| (q.table.as_str(), q.column.as_str()))
            .collect();
        assert_eq!(cols, vec![("a", "x"), ("b", "y"), ("c", "z"), ("d", "w")]);
    }

    #[test]
    fn identifiers_may_contain_digits_underscores_and_leading_underscore() {
        let j = parse("_t1.col_2 = T_3.x9");
        assert_eq!(j.conjuncts[0].lhs.table, "_t1");
        assert_eq!(j.conjuncts[0].lhs.column, "col_2");
        assert_eq!(j.conjuncts[0].rhs.table, "T_3");
        assert_eq!(j.conjuncts[0].rhs.column, "x9");
    }

    #[test]
    fn rejects_empty_input() {
        let err = JoinExpr::parse("").unwrap_err();
        assert!(err.message.contains("identifier"));
        assert_eq!(err.at, 0);
    }

    #[test]
    fn rejects_whitespace_only_input() {
        let err = JoinExpr::parse("   ").unwrap_err();
        assert!(err.message.contains("identifier"));
    }

    #[test]
    fn rejects_identifier_starting_with_digit() {
        let err = JoinExpr::parse("1a.x = b.y").unwrap_err();
        assert!(err.message.contains("identifier"));
    }

    #[test]
    fn rejects_missing_column_after_dot() {
        let err = JoinExpr::parse("a. = b.y").unwrap_err();
        assert!(err.message.contains("identifier"));
    }

    #[test]
    fn rejects_missing_operator() {
        // After the lhs qcol the parser expects an operator, not another qcol.
        let err = JoinExpr::parse("a.x b.y").unwrap_err();
        assert!(err.message.contains('='));
    }

    #[test]
    fn rejects_missing_rhs() {
        let err = JoinExpr::parse("a.x =").unwrap_err();
        assert!(err.message.contains("identifier"));
    }

    #[test]
    fn rejects_two_conjuncts_without_and() {
        // A second conjunct must be separated by `AND`; bare juxtaposition is
        // an error pointing at the missing keyword.
        let err = JoinExpr::parse("a.x = b.y c.z = d.w").unwrap_err();
        assert!(err.message.contains("AND"));
    }

    #[test]
    fn error_offset_points_at_failing_token() {
        // The unknown operator sits at byte 4.
        let err = JoinExpr::parse("a.x ~ b.y").unwrap_err();
        assert_eq!(err.at, 4);
    }
}
