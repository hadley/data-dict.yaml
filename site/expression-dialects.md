# Expression dialects

**Status: proposal.** Nothing on this page is part of the spec yet. It proposes an extension to the [expression language](expressions.md) and a sketch of SQL translation; if adopted, the normative text moves into `expressions.md` and this page becomes design rationale.

## Motivation

The expression language is SQL-flavoured, but the people writing data dictionaries mostly live in R, pandas, or polars. Today they must mentally translate `!is.na(x)` into `x IS NOT NULL` and `x %in% c('a', 'b')` into `x IN ('a', 'b')` before writing it down. The translation is small but constant, and it makes every expression feel like someone else's language.

This proposal lets an author write an expression in the dialect they think in. It is deliberately *not* four languages: there is **one language with one AST, one set of semantics, and one type checker** — the dialects are alternative spellings of the same tokens and functions, with one deliberate exception ([modulo](#modulo), where the dialects truly disagree). `x %in% c('a', 'b')`, `x in ('a', 'b')`, and `is_in(x, 'a', 'b')` parse to the identical AST node and are indistinguishable from that point on. Spellings may be mixed freely in one expression, because there is nothing to mix — the parser accepts the union.

Two consequences of the one-AST rule are worth stating up front:

* **Semantics never follow the spelling.** Three-valued logic, null propagation, `CHECK`-style assertion semantics, and case-insensitive keywords apply no matter which dialect an expression looks like. `x == None` yields null (and the assertion passes), even though the Python it resembles would say `False`.
* **Everything downstream is dialect-blind.** Type checking, diagnostics, and the [SQL translation](#lowering-to-sql) below are defined on the AST, so no dialect needs its own rules. Diagnostics point at the source span as the author wrote it.

## What changes for SQL

Almost everything in this proposal gives meaning to syntax that is an error today, so existing dictionaries keep parsing and keep meaning what they meant. The exceptions and near-exceptions, gathered in one place:

**The one genuinely breaking change: `None` and `NA` become reserved words.** A dictionary whose expressions refer to a bare column named `NA`, `na`, `None`, or `none` (reserved words are case-insensitive) stops parsing; the fix is backticks. Nothing else in the proposal can change the parse or the meaning of a currently-valid expression.

**Syntax that a SQL author already has expectations for, which this proposal assigns a different meaning.** Each of these is a syntax error today, so no existing expression changes — but a SQL reader's instinct will be wrong:

| Syntax | SQL instinct | Meaning here |
|--------|--------------|--------------|
| `"end date"` | quoted identifier | string literal — identifiers use backticks |
| `a \|\| b` | string concatenation | `OR` |
| `x % y` | truncated modulo (sign of dividend) | floored modulo (sign of divisor); `MOD(x, y)` keeps the SQL semantics |
| `s ~ p` | regex match (PostgreSQL) | still a syntax error — `~` is prefix `NOT` only; use `SIMILAR TO` |
| `a & b`, `a \| b` | bitwise (MySQL and others) | `AND` / `OR` |

: {tbl-colwidths="[20,35,45]"}

**Previously-rejected syntax that becomes legal with its obvious meaning**, no SQL conflict: `==`, `!`, `&&`, `%%`, `%in%`, chained comparisons (`lo <= x <= hi`), method calls (`x.trim().length()`), the `[…]` and `c(…)` list forms after `IN`, and the new function names. A SQL author can ignore all of it and write exactly what they write today.

## Operator aliases

| Canonical | New spellings | Source |
|-----------|---------------|--------|
| `x = y` | `x == y` | Python, R, pandas, polars |
| `NOT x` | `!x`, `~x` | R; pandas, polars |
| `x AND y` | `x & y`, `x && y` | pandas/polars; R |
| `x OR y` | `x \| y`, `x \|\| y` | pandas/polars; R |
| `x IN (…)` | `x %in% (…)` | R |

: {tbl-colwidths="[25,30,45]"}

Notes:

* `!=` already has two spellings (`!=`, `<>`); no change.
* Python's keyword forms — `and`, `or`, `not`, `in`, `is` — already parse, because keywords are case-insensitive. So do `x is not None` and `x not in (…)` once the [null spellings](#literals) below land: the existing `IS [NOT]` and `[NOT] IN` grammar carries them.
* **`||` is OR, not string concatenation.** The language has no concatenation, so the SQL reading has nothing to attach to; a SQL author who writes `first || last` gets a type error (OR on strings), which should carry the hint `Hint: ||: is OR here, not string concatenation.`
* Each new spelling takes the **canonical operator's precedence**, not its home dialect's. See [precedence divergences](#precedence-divergences).

### Modulo

Modulo is the one place where dialect support adds an **operation** rather than a spelling, because the dialects genuinely disagree on what modulo means:

* **`MOD(x, y)` is unchanged**: truncated modulo, sign of the dividend, `MOD(-7, 3)` = `-1` — the C and SQL convention the spec already has.
* **`x % y` and `x %% y` are new, and floored**: sign of the divisor, `-7 % 3` is `2` and `7 % -3` is `-2` — the Python, R, pandas, and polars convention. Formally, `x - y * FLOOR(x / y)`.

Both are null when `y` is zero, like division. The two agree whenever the operands share a sign (in particular for the overwhelmingly common non-negative case, `MOD(minutes, 15)` = `minutes % 15`); they differ only when the signs differ.

Making `%` an alias of `MOD` would hand dividend-sign semantics to authors arriving from three divisor-sign languages; redefining `MOD` would do the reverse to SQL authors. Two operators, each faithful to its home spelling, is the only assignment that surprises nobody — at the cost that the docs for each must cross-reference the other, since nothing about the spellings hints that they differ.

`%` and `%%` sit at multiplicative precedence, with `*` and `/` (the Python and SQL position).

## Literals

| Canonical | New spellings | Source |
|-----------|---------------|--------|
| `NULL` | `None`, `NA` | Python; R |
| `'text'` | `"text"` | Python, R, pandas, polars |
| `TRUE` / `FALSE` | — (`True`, `true` already parse) | |

: {tbl-colwidths="[25,35,40]"}

* `None` and `NA` are full synonyms for `NULL` everywhere it can appear, including after `IS [NOT]` — so `x is None` and `x IS NOT NA` both parse. They join the reserved words, and like all keywords they are case-insensitive: a column named `NA`, `na`, or `none` can no longer be referred to bare. Backtick quoting is the escape hatch, as for any awkward name.
* Double-quoted strings follow exactly the single-quote rules: `"O""Brien"` doubles the quote, and there are no backslash escapes. The other quote character needs no escaping (`"it's"`, `'say "hi"'`), which is the main reason to want both. Backticks quote *identifiers* and double quotes now make *strings* — the SQL reading of `"…"` as an identifier is rejected. Each string uses one quote character; the two styles don't mix within a literal.
* R's typed variants (`NA_character_`, `NA_real_`, …) are not included; `NA` is already typeless here.

## Function aliases

All aliases are case-insensitive, like every function name. Each row is one AST node; arity and types are those of the canonical form.

| Canonical | New spellings | Source |
|-----------|---------------|--------|
| `LENGTH(s)` | `len`, `nchar`, `len_chars` | Python; R; polars |
| `LOWER(s)` | `tolower`, `to_lowercase` | R; polars |
| `UPPER(s)` | `toupper`, `to_uppercase` | R; polars |
| `TRIM(s)` | `trimws`, `strip`, `strip_chars` | R; Python; polars |
| `STARTS_WITH(s, p)` | `startswith` | Python; R's `startsWith` via case folding |
| `ENDS_WITH(s, p)` | `endswith` | Python; R's `endsWith` via case folding |
| `CEIL(x)` | `ceiling` | R |
| `x IS NULL` | `is_null(x)`, `isna(x)` | polars; pandas |
| `x IS NOT NULL` | `is_not_null(x)`, `notna(x)` | polars; pandas |
| `x IN (a, b, …)` | `is_in(x, a, b, …)` | polars (variadic, not a list argument) |
| `x BETWEEN lo AND hi` | `between(x, lo, hi)`, `is_between(x, lo, hi)` | R (dplyr), pandas; polars |
| `CASE WHEN c THEN a ELSE b END` | `ifelse(c, a, b)`, `if_else(c, a, b)` | R; dplyr |
| `interval(n, weeks)` etc. | `seconds(n)`, `minutes(n)`, `hours(n)`, `days(n)`, `weeks(n)` | R (lubridate) |

: {tbl-colwidths="[30,35,35]"}

`ABS`, `FLOOR`, and `ROUND` are already spelled the same in all four dialects and need nothing.

Two aliases carry a documented semantic divergence from their namesakes:

* **`ifelse(c, a, b)` is `CASE`, including its null handling**: a null condition selects `b` (the `ELSE`), where R's `ifelse(NA, a, b)` returns `NA`. Polars' `when/then/otherwise` matches `CASE`, so the alias sides with polars and SQL.
* **`length` keeps its character-count meaning.** In R, `length()` of a string is `1` (vector length); `nchar` is the faithful R name, and `length` remains the canonical SQL one.

R's dotted names (`is.na`, `Sys.time`) are not aliases: with `.` becoming the [method-call](#method-calls) operator, `is.na(x)` can't be an identifier, and gets a hint instead.

## Method calls

Any function — canonical or alias — may also be written as a postfix method on its first argument: `x.f(a, …)` is sugar for `f(x, a, …)`, one rule for every function (uniform function-call syntax), so both `x.length()` and `length(x)` are `LENGTH(x)`. Calls chain left to right: `name.trim().lower()` is `LOWER(TRIM(name))`.

```yaml
- assert: postcode.trim().length() <= 10
- assert: sku.startswith('NZ-')
- assert: minutes.between(0, 59)
```

* **Parentheses are required.** `x.length` without them is a syntax error (with a hint) — there is no property access, which keeps `.` unambiguous.
* **The `str` accessor is accepted and ignored.** pandas and polars reach string methods through `.str` (`x.str.len()`, `x.str.len_chars()`), so the qualifier parses as an inert prefix: `x.str.len()` means `x.len()`. There is no `.dt` — the language has no datetime functions to sit behind it.
* **The receiver is any primary**: `(a + b).abs()`, `'abc'.length()`, and `COLUMNS('q[4-8]').is_not_null()` all work, the last because `COLUMNS(...)` stands in for a column reference. Method calls bind tightest, so `-x.abs()` is `-(x.abs())`.
* **Numbers still lex first.** A `.` is part of a number literal only between digits, so `2.round()` and `3.14.round(1)` parse as methods on the literals.
* **Uniformity admits silly spellings.** `c.ifelse(a, b)` is legal and `x.now()` is a well-formed parse that fails arity checking. One rule with odd corners beats a curated method list; style guides, not grammar, discourage them.

This is where the polars function aliases pay off twice: `is_null`, `is_in`, and `is_between` were chosen as function names precisely so that the method spellings — `x.is_null()`, `x.is_in('a', 'b')`, `x.is_between(lo, hi)` — land on them and read as native polars.

## Chained comparisons

`lo <= x <= hi` becomes legal, desugaring to `lo <= x AND x <= hi` — Python and pandas `query()` idiom. A chain must run in one direction: every link is drawn from `<`/`<=`, or every link from `>`/`>=`. Mixed chains (`a < b > c`) and equality chains (`a = b = c`) stay syntax errors, so a chain always reads as a range. This replaces the current blanket "comparisons don't chain" rule; the diagnostic for the still-illegal shapes should keep pointing at `BETWEEN`.

## The `IN` list

The list after `IN` (or `%in%`, spelled any way) gains two forms alongside `(a, b, c)`:

| Form | Source |
|------|--------|
| `x in (a, b, c)` | SQL, Python (a tuple, literally valid Python) |
| `x in [a, b, c]` | polars `is_in([...])`, Python list |
| `x %in% c(a, b, c)` | R |

: {tbl-colwidths="[40,60]"}

`c` is **not** a reserved word: `c(…)` is recognized only in list position, immediately after an `IN` spelling. A column named `c` and a function call `c(…)` elsewhere mean what they always did (the latter stays an unknown-function error).

## Precedence divergences

The new spellings adopt the canonical operator's precedence. That is a *choice*, and it diverges from some home dialects:

* **R agrees almost everywhere.** R's `!` sits below comparisons, `&`/`&&` above `|`/`||`, exactly mirroring `NOT`/`AND`/`OR` — an R expression means the same thing here. The one exception: R's `%%` binds *tighter* than `*`, so `a * b %% c` parses as `a * (b %% c)` in R but `(a * b) % c` here. Parenthesize.
* **Python agrees everywhere that matters**: `not`/`and`/`or`/`in`/`is` and `%` all land on their Python tiers.
* **pandas and polars disagree, mostly benignly.** Their `&`, `|`, `~` are bitwise-tight, which forces `(a > 1) & (b < 2)` at home. Here `&`/`|` are loose, so the habitual parentheses are redundant but harmless, and the unparenthesized form that would crash in pandas just works. The genuine footgun is `~a == b`: `(~a) == b` at home, `~(a = b)` here. It's rare (both readings need a boolean `a`), can't be detected, and is called out in docs rather than legislated around.

The precedence table in `expressions.md` gains the new spellings in place: `!`/`~` on the `NOT` tier, `&`/`&&` on `AND`, `|`/`||` on `OR`, `%`/`%%` with `*`/`/`, `%in%` with `IN`.

## Diagnostics

No new problem codes: a bad dialect expression is still malformed (S19), unknown-column (S20), or ill-typed (S21). The wins are targeted hints on near-misses, following the existing `expected`/`message` conventions:

| Author wrote | Hint |
|--------------|------|
| `first \|\| last` on strings | `Hint: \|\|: is OR here, not string concatenation.` (S21) |
| `pl.col('x')` | `Hint: Refer to columns by bare name: x.` — desugars to `col(pl, 'x')`, caught as an unknown function (S21) |
| `x.length` without parentheses | `Hint: Methods need parentheses: x.length().` (S19) |
| `is.na(x)` | `Hint: Write isna(x) or x.is_null().` (S19 — `is` is a reserved word, so the parse fails at the `.`) |
| `a < b > c` | `Hint: A chain must run in one direction; write b BETWEEN a AND c.` (S19) |

: {tbl-colwidths="[40,60]"}

## Out of scope

* **Attribute access.** `.` exists only as a method call: there are no properties (`x.length` needs its parentheses), and no namespaces beyond the inert `str` qualifier — `pl.col('x')`, `x.dt.year`, and R's dotted names (`is.na`, `Sys.time`) get hints, not meanings.
* **Keyword arguments** (`timedelta(weeks=2)`, `pl.duration(weeks=2)`); lubridate-style `weeks(2)` covers the need.
* **Exponentiation** — R's `^` is Python's XOR, Python's `**` is nothing in R; adding either invites the confusion this proposal otherwise avoids.
* **String concatenation**, simple-form `CASE`, aggregates: unchanged from the base language.
* **NaN.** pandas' NaN-based comparisons (`NaN == x` is `False`, not null) contradict three-valued logic and are a data-representation issue, not a syntax one.

## Grammar changes

The delta against the grammar in `expressions.md`:

```text
or_expr        := and_expr (("OR" | "|" | "||") and_expr)*
and_expr       := not_expr (("AND" | "&" | "&&") not_expr)*
not_expr       := ("NOT" | "!" | "~") not_expr | predicate
predicate      := additive ( eq_cmp additive
                           | (lt_cmp additive)+
                           | (gt_cmp additive)+
                           | "IS" ["NOT"] null
                           | ["NOT"] "BETWEEN" additive "AND" additive
                           | ["NOT"] in_op in_list
                           | ["NOT"] "LIKE" additive
                           | ["NOT"] "SIMILAR" "TO" additive )?
multiplicative := unary (("*" | "/" | "%" | "%%") unary)*
unary          := "-" unary | postfix
postfix        := primary ("." method)*
method         := ["str" "."] IDENT "(" (expr ("," expr)*)? ")"
eq_cmp         := "=" | "==" | "!=" | "<>"
lt_cmp         := "<" | "<="
gt_cmp         := ">" | ">="
in_op          := "IN" | "%in%"
in_list        := "(" expr ("," expr)* ")"
                | "[" expr ("," expr)* "]"
                | "c" "(" expr ("," expr)* ")"
null           := "NULL" | "None" | "NA"
literal        := number | string | "TRUE" | "FALSE" | null
string         := single- or double-quoted, embedded quote doubled
```

A `(lt_cmp additive)+` run of length one is an ordinary comparison; longer runs desugar to the `AND` of adjacent pairs. A method call desugars to the plain call with the receiver prepended: `x.f(a, …)` ≡ `f(x, a, …)`. Lexing tries `%in%`, then `%%`, then `%`; a `.` is part of a number literal only between digits. Reserved words grow by `None` and `NA`.

## Lowering to SQL

Because every dialect lands on one typed AST, translation to SQL is a single function of the AST — the dialects impose no cost here. Two consumers motivate it:

* **A violations query** for a table `t`: `SELECT * FROM t WHERE (expr) IS FALSE`. `IS FALSE` is exactly assertion semantics inverted — null and true both pass, so only false rows surface.
* **A `CHECK` constraint**: `CHECK (expr)` verbatim, since SQL `CHECK` already passes null.

The translator works on the *typed* AST, which lets it be explicit where the engine would guess or crash:

* **Totality is preserved with `NULLIF`.** The language never errors at runtime — division and modulo by zero are null — but Postgres raises. Every emitted divisor is wrapped: `x / NULLIF(y, 0)`.
* **Temporal literals become explicit casts.** Where the type checker admitted `birthdate >= '2000-01-01'`, the translator emits `birthdate >= CAST('2000-01-01' AS DATE)` rather than relying on engine coercion.
* **`COLUMNS(...)` expands at translation time.** The dictionary knows the column list (it must, for S22), so the translator emits the explicit `AND`-conjunction and no engine needs DuckDB's `COLUMNS`.

Node-by-node, for the two initial targets:

| AST node | DuckDB | PostgreSQL |
|----------|--------|------------|
| literals, columns | verbatim; identifiers double-quoted (`"end date"`) | same |
| `=` `!=` `<` `<=` `>` `>=` | verbatim (`!=` as `<>`) | same |
| `AND` `OR` `NOT` | verbatim | same |
| `x / y` | `x / NULLIF(y, 0)` | same |
| `%` (floored) | `((x % NULLIF(y,0)) + y) % NULLIF(y,0)` | same (`%` truncates in both engines) |
| `IS [NOT] NULL`, `BETWEEN`, `IN` | verbatim | same |
| `LIKE` | verbatim | same |
| `SIMILAR TO` | `regexp_full_match(s, p)` | `s ~ ('^(?:' \|\| p \|\| ')$')` |
| `CASE` | verbatim | same |
| `LENGTH` | `length(s)` — characters in both | same |
| `LOWER` `UPPER` `TRIM` | verbatim | same |
| `STARTS_WITH` | `starts_with(s, p)` | `starts_with(s, p)` |
| `ENDS_WITH` | `ends_with(s, p)` | `right(s, length(p)) = p` |
| `ABS` `FLOOR` `CEIL` | verbatim | same |
| `ROUND(x, d)` | `round(x, d)` — half away from zero | `round(CAST(x AS numeric), d)` — the cast forces half-away; `round(double)` is half-even |
| `MOD(x, y)` (truncated) | `x % NULLIF(y, 0)` — engine `%` already truncates | same |
| `NOW()` | `now()` — transaction-scoped, so fixed per evaluation as specified | same |
| `interval(n, unit)` | `n * INTERVAL 1 SECOND` (scaled per unit) | `n * INTERVAL '1 second'` |
| date/datetime ± interval | verbatim `+`/`-` | same |
| `COLUMNS(...)` | expanded to a conjunction before emission | same |

: {tbl-colwidths="[22,39,39]"}

Known gaps to resolve during implementation:

* **Regex flavour.** `SIMILAR TO` patterns are RE2. DuckDB's regex *is* RE2, so translation is exact. Postgres regexes are close but not identical (inline `(?i)` works; some escapes differ) — the Postgres translation is best-effort and should say so, or restrict patterns to a common subset.
* **Fractional intervals.** `interval(1.5, hours)` multiplies an interval by a non-integer; Postgres supports it, DuckDB's `interval * double` support needs verifying, else scale to seconds first.
* **String comparison collation.** The language compares by code point; an engine's collation may differ. Emitting `COLLATE` clauses is the fix if it bites.

## Open questions

1. **Should `NA` really be reserved case-insensitively?** `na` is a plausible column name in the wild. Options: accept the cost (backticks exist), or make `None`/`NA` the language's only case-*sensitive* keywords, matching their home spellings. The second is more surprising as a rule but bites fewer files.
2. **Alias breadth.** The function-alias table is a judgement call; every row is cheap individually but together they enlarge the surface a reader must recognize. Happy to cut (e.g. `isna`/`notna`) or extend on review.
3. **The inert `str` qualifier.** Accepting-and-ignoring `.str` is what lets real pandas/polars snippets (`x.str.len()`) paste in unchanged, but a token that parses to nothing is a strange thing to specify. The alternative is a hint (`Hint: Drop .str: x.len().`), which costs pasters one edit.
4. **Canonical formatting.** With many spellings for one AST, a `data-dict fmt` that rewrites expressions to canonical SQL spelling (or to a chosen dialect) becomes attractive. Out of scope here, but the one-AST design makes it a pure pretty-printer.
