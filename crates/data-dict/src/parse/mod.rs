//! Reading an expression written in another language into the language's own
//! [surface AST](crate::assert_expr::AssertExpr).
//!
//! The mirror of [`emit`](crate::emit), and deliberately not built like it.
//! `emit` has a shared driver — [`emit`](crate::emit::emit) walks one IR and
//! calls back into a [`Target`](crate::emit::Target) for each node's spelling —
//! so a trait earns its keep there. There is no inbound equivalent: two source
//! languages share no tokenizer, no grammar, and no precedence table, so a
//! `Source` trait would have exactly one method and buy nothing. A reader is a
//! function, and [`Language`] is a name attached to one.
//!
//! # What a reader produces
//!
//! The **surface** AST, not the [typed IR](crate::assert_expr::TypedAssertion).
//! Everything downstream then works unchanged — most importantly
//! [`check`](crate::assert_expr::check), so `nchar(nope) > 0` reports the
//! ordinary S20 for an unknown column rather than needing a reader of its own to
//! resolve names. It also keeps a reader free of the table it will be checked
//! against, preserving the invariant `assert_expr` states: parsing is pure
//! syntax. That pays for itself — `as.Date("2020-01-01")` reads as a plain
//! string, and lowering turns it back into a date when it meets a `date` column,
//! by the rule the language already has.
//!
//! # Spans
//!
//! A node's span points into the **source text that was parsed**, so for a
//! reader it points into the R, not into any data-dict spelling. A node built by
//! folding several R nodes together spans from the first byte of the leftmost
//! contributing token to the last byte of the rightmost.
//!
//! # Notes
//!
//! Where a construct means something slightly different in its own language
//! than the reading it is given, the difference travels as a note. Notes are
//! collected here rather than hung on the AST, which has nowhere to put them —
//! the same shape [`Emitted`](crate::emit::Emitted) uses going the other way.
//!
//! A reader never repairs such a difference by adding a guard. See
//! `site/expression-execution.md#fidelity` for why: emitted code is for a
//! machine to run, where an added guard is invisible and welcome, but a reading
//! becomes the dictionary's own statement of the rule, which its author has to
//! be able to recognise as theirs.

pub mod r;

#[cfg(test)]
mod roundtrip;

use std::collections::BTreeSet;
use std::fmt::Display;

use crate::assert_expr::{AssertExpr, ParseError};

/// One expression, read.
#[derive(Debug)]
pub struct Parsed {
    pub expr: AssertExpr,
    /// Where the source language and the reading disagree, in a stable order.
    pub notes: Vec<&'static str>,
}

impl Parsed {
    /// A reading that has nothing to warn about — what the data-dict language's
    /// own "reader" always produces, since it is already the language.
    fn exact(expr: AssertExpr) -> Parsed {
        Parsed {
            expr,
            notes: Vec::new(),
        }
    }
}

/// The notes a reading has accumulated. Deduplicated and ordered, so one note
/// is attached once however many times its construct appears.
#[derive(Debug, Default)]
pub(crate) struct Notes(BTreeSet<&'static str>);

impl Notes {
    pub(crate) fn add(&mut self, note: &'static str) {
        self.0.insert(note);
    }

    pub(crate) fn into_vec(self) -> Vec<&'static str> {
        self.0.into_iter().collect()
    }
}

/// A language an expression can be read from.
pub struct Language {
    /// The name the spec gives it, as a `language:` value and as `--from`.
    pub name: &'static str,
    read: fn(&str) -> Result<Parsed, ParseError>,
}

impl Language {
    pub fn read(&self, source: &str) -> Result<Parsed, ParseError> {
        (self.read)(source)
    }
}

/// Every language that can be read, in a stable order. The data-dict language
/// is one of them, so nothing has to special-case the default.
pub fn languages() -> &'static [Language] {
    &[
        Language {
            name: "data-dict",
            read: |source| AssertExpr::parse(source).map(Parsed::exact),
        },
        Language {
            name: "r",
            read: r::read,
        },
    ]
}

/// The language an expression is written in when it doesn't say.
pub fn default_language() -> &'static Language {
    &languages()[0]
}

/// Resolve a `language:` value or a `--from` argument, or say why not.
pub fn resolve(name: &str) -> Result<&'static Language, String> {
    match languages()
        .iter()
        .find(|language| language.name.eq_ignore_ascii_case(name))
    {
        Some(language) => Ok(language),
        None => {
            let available = languages()
                .iter()
                .map(|language| language.name)
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "unknown expression language `{name}`; available: {available}"
            ))
        }
    }
}

/// A construct that is well-formed in its own language but has no equivalent
/// here, so the rule it states can't be recorded. Distinct from a syntax error:
/// the fix is to rewrite the rule, not to correct a typo.
pub(crate) fn untranslatable(what: impl Display, why: &str, at: usize) -> ParseError {
    ParseError {
        message: format!("{UNTRANSLATABLE}{what} — {why}"),
        at,
    }
}

/// Marks a [`ParseError`] as a construct with no equivalent rather than a
/// syntax error, so a caller can report the two differently. Stripped before
/// the message is shown.
pub(crate) const UNTRANSLATABLE: &str = "\u{0}";

/// Split an error into its message and whether it was [`untranslatable`].
pub(crate) fn classify(error: &ParseError) -> (&str, bool) {
    match error.message.strip_prefix(UNTRANSLATABLE) {
        Some(rest) => (rest, true),
        None => (error.message.as_str(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_resolves_by_its_own_name() {
        for language in languages() {
            assert_eq!(resolve(language.name).map(|l| l.name), Ok(language.name));
        }
        assert!(resolve("R").is_ok(), "matching ignores case");
    }

    #[test]
    fn an_unknown_language_lists_the_readable_ones() {
        let Err(err) = resolve("perl") else {
            panic!("no such language")
        };
        assert!(err.contains("unknown expression language"), "{err}");
        assert!(err.contains("data-dict, r"), "{err}");
    }

    #[test]
    fn the_default_is_the_language_itself() {
        assert_eq!(default_language().name, "data-dict");
        // Reading it is just parsing it, so it never has anything to warn about.
        let parsed = default_language().read("qty > 0").expect("parses");
        assert!(parsed.notes.is_empty());
    }
}
