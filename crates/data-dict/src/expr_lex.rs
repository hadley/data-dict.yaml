//! Lexical pieces shared by the expression parsers in [`crate::assert_expr`]
//! and [`crate::join_expr`].

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    /// Byte offset of the failing token (or end-of-string) within the input.
    pub at: usize,
}

/// Parse a backtick-quoted name with `pos` at the opening backtick, leaving it
/// after the closing backtick. Errors point at the opening backtick, the token
/// the reader has to fix.
pub(crate) fn parse_quoted_name(src: &[u8], pos: &mut usize) -> Result<String, ParseError> {
    let start = *pos;
    debug_assert_eq!(src.get(start), Some(&b'`'));
    *pos += 1;
    let mut name = String::new();
    loop {
        match src.get(*pos) {
            None => {
                return Err(ParseError {
                    message: "unterminated quoted name".into(),
                    at: start,
                });
            }
            Some(b'`') => {
                // A doubled backtick is a literal backtick; a lone one ends it.
                if src.get(*pos + 1) == Some(&b'`') {
                    name.push('`');
                    *pos += 2;
                } else {
                    *pos += 1;
                    break;
                }
            }
            Some(_) => {
                let ch_start = *pos;
                advance_char(src, pos);
                name.push_str(
                    std::str::from_utf8(&src[ch_start..*pos]).expect("input is valid utf-8"),
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
pub(crate) fn advance_char(src: &[u8], pos: &mut usize) {
    *pos += 1;
    while let Some(&b) = src.get(*pos) {
        if b & 0xC0 == 0x80 {
            *pos += 1;
        } else {
            break;
        }
    }
}
