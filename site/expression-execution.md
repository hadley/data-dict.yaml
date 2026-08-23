# Executing expressions

[Expressions](expressions.md) describes a language for stating a rule about the values in one table. This page describes what is done with such a rule once it has been written: **evaluating** it against data, and **translating** it into R, Python, or SQL so it can be checked somewhere else.

The two are the same thing seen from two sides. A dictionary outlives any one tool, and a rule that can only be checked by `data-dict` is a rule a team has to write twice. So an assertion is not only something the validator enforces — it is also a portable statement that can be handed to a pipeline, a test suite, or a database `CHECK` constraint.

Both start from the same place. An expression is [parsed and type-checked](expressions.md#type-checking) when the dictionary is validated, and only an expression that passes every check is ever evaluated or translated. Neither activity reports problems with the expression itself: by the time either runs, the expression is known to be well-formed, its columns resolved, and every subexpression's type known.

## Evaluation

`data-dict validate-data` evaluates each of a table's assertions against the table's data, reporting the rows that break them ([D07](validation.md#data-validation-checks)).

This is also the language's **reference implementation**. Where this page and [expressions.md](expressions.md) describe a behaviour, `validate-data` is what that behaviour means, and every [translation](#translating-expressions) is judged by whether it agrees.

### What counts as a violation

An assertion follows SQL's `CHECK` semantics, [as the language specifies](expressions.md#truth-and-null): a row **passes** when the expression is `true` **or** `null`, and only `false` is a violation. A comparison against a null column is null, so an assertion never doubles as a null check — pair it with `required` when the column must also be present.

What a violation can name depends on the assertion's [shape](expressions.md#shapes):

* A **`row`** assertion is evaluated once per row, so a report gives the number of violating rows and identifies the first few.
* An **`agg`** or **`const`** assertion is a single verdict about the whole table, so a report can only say that it is false. `COUNT(email) >= 0.9 * ROW_COUNT()` either holds or does not; no row is individually to blame.

A **mixed-grain** assertion such as `value <= 2 * MIN(value)` is a `row` assertion: its aggregate parts are folded over the whole table first, and the resulting single values are then used in the per-row pass. This is the extra pass over the column that [shapes](expressions.md#shapes) mentions, and it is why an aggregate can be compared against a row-level value at all.

### Values

Evaluation works over the language's six [types](expressions.md#types) plus null, and **numbers are integers or floats**, [as the language describes](floating-point.md#integers-and-floats) — integer arithmetic is exact, and `/` always produces a float.

Null is used for one thing only: a value that is missing or unknown. It is never used to stand for a value that arithmetic failed to produce, and never for a value that is there but isn't a number: [an infinity and a NaN are values](floating-point.md#non-finite).

#### Arithmetic with no result {#no-result}

One situation leaves an expression with no answer to give. It yields no value; it is reported, and the assertion's verdict for that table is withdrawn rather than guessed at — a `D09` replaces the `D07` that would otherwise be reported.

**Integer overflow** ([D09](validation.md#data-validation-checks)), when integer arithmetic leaves the 64-bit range — in `+`, `-` and `*`, in `ABS` and unary minus at the extreme negative integer, in an `interval` count or a shift by one, and in `SUM` as it accumulates. Wrapping or saturating would mean the arithmetic no longer computes what the expression says. Floats are unaffected: they overflow to `INF`, which is [a value the language has](floating-point.md#non-finite), so the expression still reaches a verdict.

This does mean evaluation is not total — some data can stop an assertion from reaching a verdict. That is a deliberate trade. A rule that cannot be computed has not been checked, and saying so is more useful than a pass nobody earned.

#### Non-finite values {#non-finite}

Division by zero is not among the situations above: `7 / 0` is `INF`, `0 / 0` is a NaN, `MOD(x, 0)` is a NaN. Nothing is reported, and evaluation carries on. A NaN or an infinity read from the data is treated the same way as one the arithmetic produced. [Floating point](floating-point.md#evaluation) covers both, and why comparing against a NaN gives `false` rather than null.

### Time

`NOW()` is [fixed for the whole evaluation](expressions.md#now), so two `NOW()`s in one expression always agree. `validate-data` goes one step further and binds it once for the **whole run**, so every assertion in every table of a single validation shares one reading of the clock. A run therefore describes the data as of one instant, and cannot report a pair of results that no single moment could produce.

### Patterns

`SIMILAR TO` and `COLUMNS('<regex>')` take [RE2](https://github.com/google/re2/wiki/Syntax) regular expressions, which the reference implementation matches exactly. `LIKE` is defined in terms of its own two wildcards and does not depend on a regex flavour.

A literal pattern is compiled when the dictionary is validated, so a malformed one is an S21 at the spec level. A pattern read from a column can only be compiled once the data is in hand, and one that doesn't compile is [reported as D08](validation.md#data-validation-checks) rather than treated as a non-match — a non-match would make the row *pass*, quietly retiring the rule on exactly the rows whose pattern is broken.

### When an assertion can't be run

An assertion can only be evaluated if the columns it names can be read as the types the dictionary declares for them, and if the patterns it matches against compile. When either fails, it is reported as an error ([D08](validation.md#data-validation-checks)).

Not as a warning, and not as a pass. An assertion that was never evaluated has not been satisfied, and treating it as satisfied is the one outcome that hides the problem: the dictionary would go on claiming a rule the data was never held to. Reporting it says what is actually true — that this rule is currently unenforceable, and either the declared type or the data has to change.

Most type disagreements never reach this point, since a column whose data contradicts its declared type is already an error at the metadata level (M01). D08 is what remains: a column whose type is right in kind but whose values can't be brought into the [value model](#values) — a number held in a decimal too wide for exact 64-bit arithmetic, say — and a [pattern taken from the data](#patterns) that isn't a valid regular expression.

## Translating expressions

`data-dict translate` renders an assertion as code in another language. The output is a **bare predicate**, not a runnable script: it is the expression, spelled for the target, ready to be dropped into a `filter()`, a `WHERE` clause, or a test. Loading the data, iterating over a dictionary's assertions, and reporting results are the caller's business — see [embedding a predicate](#embedding) for the idiom each target uses.

### Targets

A target is named `family(dialect)`. A bare family name means that family's default dialect.

| Family | Dialects | Bare form means |
|--------|--------------------------------|-----------------|
| `R` | `base`, `tidyverse`, `data.table` | `R(base)` |
| `Python` | `polars`, `pandas` | `Python(polars)` |
| `SQL` | `ANSI`, `duckdb`, `postgres` | `SQL(ANSI)` |

: {tbl-colwidths="[15,45,40]"}

Seven of the eight targets are defined by something outside this specification: `R(tidyverse)` means what dplyr and stringr do, `Python(polars)` what polars does, `SQL(duckdb)` what DuckDB does. Those are versioned, testable things, and the translation for each is fixed by agreement with the reference implementation rather than by wording here.

`SQL(ANSI)` is the exception, and [has a grammar of its own](#ansi) — there is no "ANSI engine" to define it.

### Sources

Translation also runs the other way. An expression [tagged with a `language`](spec.md#other-languages) is *read* from that language into the data-dict language, and `translate --from` reads an ad-hoc one the same way.

A source is named by family alone, where a target is named `family(dialect)`. The asymmetry is real rather than an oversight: an emitter has to choose one spelling and so must be told which, while a reader can accept every spelling in the family at once. `nchar` and `str_length` are different names, so the text already says which idiom it is; there is no R expression whose meaning depends on being told it is base rather than tidyverse.

| Family | Written | Reads |
|--------|---------|-------|
| data-dict | `data-dict` | the language itself |
| R | `r` | every spelling the three `R(...)` targets emit |

: {tbl-colwidths="[20,20,60]"}

The R surface is exactly what `R(base)`, `R(tidyverse)` and `R(data.table)` emit, and no more. That is a deliberate bound: it makes the surface a finite, testable list rather than "R", and it makes the round trip a property that can be checked — every expression this specification can emit as R must read back as itself.

`data-dict` is a target as well as a source. It has no dialects, so it is written bare, and it is not emitted unless it is asked for: `translate --target data-dict` is how the data-dict spelling of an expression written elsewhere is obtained.

### Column references

Each family writes a column reference the way code in that family usually does, so the output drops into the idiom without editing:

| Family | Reference | Fits |
|--------|-----------|------|
| `R` | `postcode` | `filter()`, `dt[...]`, `with()` |
| `Python(polars)` | `pl.col("postcode")` | any polars expression context |
| `Python(pandas)` | `postcode` | `DataFrame.query()` / `eval()` |
| `SQL` | `"postcode"` | any clause |

: {tbl-colwidths="[22,28,50]"}

`Python(pandas)` has a second form, selected with `--frame`, that writes `df["postcode"]` for use outside `query()`.

### Fidelity {#fidelity}

Emitted code is required to agree with the [reference implementation](#evaluation). Where a target cannot be made to agree, that is stated rather than hidden. Every mapping from an expression's construct to a target's spelling carries one of four classifications:

| Class | Meaning |
|-------|---------|
| **Exact** | The same result on every input. |
| **Guarded** | Exact, but only because the translation adds code the expression didn't ask for — a null guard on a membership test, a cast that fixes rounding. The guard is part of the mapping. |
| **Divergent** | Agrees except on a documented edge, accepted because the exact form would be disproportionately convoluted. Using such a construct attaches a note to that translation. |
| **Unsupported** | The target cannot express it. That target reports the refusal and its reason instead of code; every other target still translates. |

: {tbl-colwidths="[18,82]"}

Refusal is per target, never per expression. A rule that can't be written in `SQL(ANSI)` still translates to the other seven.

The same scale grades a [reading](#sources), with two of the four classes unreachable. `nchar(x)` is Exact. `round(x)` is Divergent, since R rounds halves to even; so is a bare `x %in% c(1, 2)`, since R's `%in%` answers `FALSE` for an `NA` subject where the language's `IN` gives null.

**A reading is never Guarded.** Guarded means the translation adds code the expression didn't ask for, and a reader never does that: it records what the author wrote, and adding a guard would make the dictionary state a rule nobody typed. Where a construct's meaning differs, the difference is reported rather than silently repaired. This is the one place the two directions are not mirror images, and the reason is that they are read by different audiences — emitted code is for a machine to run, where a guard is invisible and welcome, while a reading becomes the dictionary's own statement of the rule, which a person has to be able to recognise as theirs.

**A reading is never Unsupported either.** Outbound, one target's refusal costs nothing, because the other seven still translate. Inbound there is nowhere else to go: a construct with no reading is a rule the dictionary cannot state at all, so it is a validation error ([S35](validation.md#spec-validation-checks)) and the dictionary does not validate until the rule is rewritten. So anything that reaches an evaluation, a translation, or an export was read Exactly or Divergently.

A Divergent reading is reported as a warning ([S36](validation.md#spec-validation-checks)). It is a difference between what the author wrote and what the dictionary will enforce, it is knowable before any data is read, and nothing else would surface it.

#### Standing divergences

These differences are broad enough to be worth naming here rather than only in a per-target table.

**Shifting a date.** [The language gives a datetime](expressions.md#arithmetic), matching DuckDB and PostgreSQL, so the SQL targets are exact. R and Python are not: `as.Date("2020-01-01") + as.difftime(12, units = "hours")` and `datetime.date(2020, 1, 1) + timedelta(hours=12)` both return the *same date*, discarding the twelve hours without a warning. Those targets therefore promote the date before shifting — `as.POSIXct(...)`, `datetime.combine(d, time.min)` — which is Guarded rather than Divergent, since the promoted form is exact. An emitter may skip the promotion when the interval is a whole-day literal, where the bare form already agrees.

**Rounding.** The language rounds halves away from zero, [as `ROUND` specifies](expressions.md#round). R, Python and pandas round halves to even, and matching the language exactly would mean replacing every `round` call with several lines of arithmetic. Those targets emit the native call, and differ only on a value that is exactly a half at the digit being rounded. The SQL targets are exact, [by casting first](#why-these-spellings).

**Regular expression flavour.** The language uses RE2. So do polars and DuckDB, which are therefore exact. stringr matches with ICU, base R's `grepl` with PCRE, and Python's `re` with its own flavour; all three accept the common syntax and differ only in corners. Where that matters, the explicit list form of `COLUMNS(...)` avoids the regex entirely.

**Empty and all-null aggregates.** The language returns null when there is nothing to fold, and an assertion [passes vacuously](expressions.md#empty-input) as a result. R, polars and pandas return the fold's identity instead — `0` for `SUM`, `false` for `ANY`, `true` for `ALL` — and R gives `Inf`/`-Inf` for `MIN`/`MAX`, which the language gives only for a column that actually contains one. So `SUM(qty) > 0` passes here on an empty table and fails in R. The SQL targets agree with the language, `ANY`/`ALL` by [an explicit guard](#why-these-spellings).

**Arithmetic with no result.** The language [reports](#no-result) an integer overflow rather than producing a value, and no target can be made to do the same, because raising is a statement and a translation is an expression. The disagreements differ per target: PostgreSQL and DuckDB raise, R has no 64-bit integer at all and gives a double, Python's integers are unbounded, and polars and pandas wrap at 64 bits. Every target therefore carries a note, and a dictionary whose data trips it should be trusted only through `validate-data`.

Division by zero splits the targets the other way. [The language gives an infinity or a NaN](floating-point.md#non-finite), and so do DuckDB, R, polars and pandas, so those four agree. PostgreSQL raises `division_by_zero` and Python raises `ZeroDivisionError`, so those two — and `SQL(ANSI)`, which is portable only where both engines agree — refuse where the language answers.

**What a NaN means.** The language says two things about a NaN: [comparing against one gives `false`, and it is a value rather than a missing one](floating-point.md#non-finite). The targets answer both three different ways, and pandas needs a stance of its own because both its backends treat a NaN as missing. [Floating point](floating-point.md#across-languages) has the table and the reasoning.

### Selecting multiple columns

[`COLUMNS(...)`](expressions.md#selecting-multiple-columns) applies one predicate to several columns and combines the results with `AND`. Every target can express that by writing the conjunction out in full, and some can do better.

A target keeps the selection as a selection only when its idiom is a **self-contained expression** with the same combination and null semantics — that is, when the result is a value that behaves correctly wherever the caller puts it. Otherwise it expands.

| Target | `COLUMNS('q[4-8]') IS NOT NULL` |
|--------|--------------------------------|
| `R(tidyverse)` | `if_all(matches("q[4-8]"), \(x) !is.na(x))` |
| `Python(polars)` | `pl.all_horizontal(pl.col("^.*q[4-8].*$").is_not_null())` |
| everything else | the expanded conjunction |

: {tbl-colwidths="[22,78]"}

`matches()` is unanchored like the language's regex; polars anchors, so the pattern is wrapped. Both idioms are ordinary values, safe under any wrapping.

DuckDB is the interesting case, since the language's `COLUMNS` is modelled on DuckDB's. DuckDB's is a syntactic macro rather than a value: it rewrites the enclosing expression once per column, and where the results are combined with `AND` depends on the clause it appears in. Since `translate` returns a bare predicate whose eventual context it cannot see, keeping the symbolic form would let `WHERE (COLUMNS(*) IS NOT NULL) IS FALSE` mean "every column is null" instead of "some column is null". DuckDB therefore expands, with a note.

### Embedding a predicate {#embedding}

A translated assertion is a predicate; what a caller usually wants is the rows that break it. Since an assertion passes on true **or** null, the violating rows are exactly those where the predicate is `false` — which is not the same as "not true". Each target has a native way to say it:

| Target | Violating rows | Why |
|--------|----------------|-----|
| `SQL` | `SELECT * FROM t WHERE (expr) IS FALSE` | `IS FALSE` is false-only by definition; null and true both escape. |
| `R(base)` | `subset(t, !(expr))` | `subset()` keeps only `TRUE`; `!NA` is `NA` and drops out. |
| `R(tidyverse)` | `filter(t, !(expr))` | `filter()` keeps only `TRUE`; `!NA` is `NA` and drops out. |
| `R(data.table)` | `t[!(expr)]` | `NA` in `i` selects nothing. |
| `Python(pandas)` | `df.query("not (expr)")` | `query()` keeps only `True`; on nullable dtypes a null is not `True`. |
| `Python(polars)` | `df.filter(~(expr))` | `~` maps false to true and null to null; `filter` drops null. |

: {tbl-colwidths="[18,40,42]"}

These are documentation, not output — how to embed a predicate is the caller's decision, and `translate` does not presume it.

### Output

`translate` is primarily for machine consumption, so it writes JSON to standard output: one record per expression, carrying the source text, where it came from, the expression's type, the columns it uses, and one entry per target.

A translation's `fidelity` is the weakest [class](#fidelity) among the constructs it used, written `"exact"`, `"guarded"`, `"divergent"` or `"unsupported"`. The field is present only when it isn't `"exact"`, and `notes` only when `fidelity` is. An entry carrying neither is exact and has nothing to warn about.

```json
{
  "expr": "LENGTH(postcode) <= 10",
  "table": "survey",
  "type": "boolean",
  "columns": [{ "table": "survey", "column": "postcode" }],
  "translations": [
    { "target": "R(base)", "code": "nchar(postcode) <= 10L" },
    { "target": "R(tidyverse)", "code": "str_length(postcode) <= 10L" },
    { "target": "SQL(duckdb)", "code": "length(\"postcode\") <= 10" },
    { "target": "Python(pandas)", "code": "postcode.str.len() <= 10",
      "fidelity": "divergent",
      "notes": ["Assumes nullable dtypes; NaN-backed comparisons return false where the language says null."] }
  ]
}
```

The `columns` list is what makes the output composable: a caller knows which columns to select, load, or index before evaluating the predicate, without parsing the code. A target that [refuses](#fidelity) is `"unsupported"` and carries an error in place of `code`.

A record for an expression that was [read from another language](#sources) says so. `language` names it, `canonical` gives the same expression in the data-dict language, and `fidelity`/`notes` grade the reading — on the same scale as a target's, and by the same presence rule: `fidelity` only when it isn't `"exact"`, `notes` only when `fidelity` is.

```jsonc
{
  "expr": "round(score) >= 50",
  "language": "r",
  "canonical": "ROUND(score) >= 50",
  "fidelity": "divergent",
  "notes": ["R rounds halves to even, where data-dict rounds them away from zero, so results differ on an exact half."],
  "table": "survey",
  "type": "boolean",
  "columns": [{ "table": "survey", "column": "score" }],
  "translations": [ /* as above */ ]
}
```

### One expression at a time

The unit of translation is one expression. By default every assertion in the dictionary is translated; `--table` narrows that to one table, and `--expr` translates an ad-hoc expression instead.

An `--expr` expression is parsed, resolved and type-checked exactly like an assertion, with one relaxation: it need not be boolean. `a + b` translates, and its type is reported. Its column names resolve against one table — the only table if the dictionary has one, and otherwise the table named by `--table`.

`--from` says what language `--expr` is written in. It applies to `--expr` alone: a dictionary's assertions each carry their own [`language`](spec.md#other-languages), which is the author's statement about the author's file, and no flag overrides it — a dictionary that validated differently depending on how the command was invoked would be no dictionary at all.

## `SQL(ANSI)` {#ansi}

`SQL(ANSI)` is the portable SQL target: the one to use when the destination engine isn't known, or when the same text has to run on more than one.

Every other target is defined by a real implementation. This one has none, so it is defined here instead, as a fixed grammar and a fixed table of spellings. That the output is *portable* is a claim about the world, tested by running it on more than one engine; that a given expression translates to a given string is settled by this section.

A spelling is admitted to the table when DuckDB and PostgreSQL both accept it with the same meaning. That criterion is how entries are chosen — it is not what they mean. Once an entry is here it stays until this page changes, so an engine release cannot alter what `data-dict translate` produces.

### Output grammar

The emitter produces this subset, and nothing outside it:

```text
expr        := or_expr
or_expr     := and_expr ("OR" and_expr)*
and_expr    := not_expr ("AND" not_expr)*
not_expr    := "NOT" not_expr | predicate
predicate   := additive ( cmp additive
                        | "IS" ["NOT"] "NULL"
                        | ["NOT"] "BETWEEN" additive "AND" additive
                        | ["NOT"] "IN" "(" expr ("," expr)* ")"
                        | ["NOT"] "LIKE" string ["ESCAPE" string] )?
additive    := multiplicative (("+" | "-") multiplicative)*
multiplicative := unary (("*" | "/") unary)*
unary       := "-" unary | primary
primary     := literal | column | funcall | cast | case
             | "CURRENT_TIMESTAMP" | "(" expr ")"
cast        := "CAST" "(" expr "AS" type ")"
type        := "DOUBLE PRECISION" | "NUMERIC"
funcall     := FUNC "(" (expr ("," expr)*)? ")"
             | "COUNT" "(" "*" ")"
             | "COUNT" "(" "DISTINCT" expr ")"
case        := "CASE" ("WHEN" expr "THEN" expr)+ ["ELSE" expr] "END"
cmp         := "=" | "<>" | "<" | "<=" | ">" | ">="
literal     := integer | decimal | string | "TRUE" | "FALSE" | "NULL"
             | "DATE" string | "TIMESTAMP" string | "INTERVAL" string
column      := '"' ( [^"] | '""' )+ '"'
FUNC        := "CHAR_LENGTH" | "LOWER" | "UPPER" | "TRIM"
             | "ABS" | "FLOOR" | "CEILING" | "ROUND" | "MOD"
             | "MIN" | "MAX" | "SUM" | "AVG" | "COUNT"
```

Columns are always quoted, so a name that collides with a keyword or differs only in case is safe. `!=` is never emitted, only `<>`; `CEIL` is spelled `CEILING`, and `CURRENT_TIMESTAMP` takes no parentheses.

### Spellings

`x`, `y`, `s` and `p` stand for already-translated subexpressions.

| Expression | `SQL(ANSI)` | Class |
|------------|-------------|-------|
| `-x` | `-x` | Exact |
| `x + y`, `x - y`, `x * y` | `x + y`, `x - y`, `x * y` | Exact |
| `x / y`, floats involved | `x / y` | Divergent |
| `x / y`, both integers | `CAST(x AS DOUBLE PRECISION) / y` | Divergent |
| `d + i`, `d - i`, `t + i`, `t - i` | `d + i`, `d - i` | Exact |
| `x = y` | `x = y` | Divergent on `number`, else Exact |
| `x != y`, `x <> y` | `x <> y` | Divergent on `number`, else Exact |
| `x < y`, `x <= y`, `x > y`, `x >= y` | same | Divergent on `number`, else Exact |
| `x IS [NOT] NULL` | `x IS [NOT] NULL` | Exact |
| `NOT x`, `x AND y`, `x OR y` | `NOT x`, `x AND y`, `x OR y` | Exact |
| `x [NOT] BETWEEN lo AND hi` | same | Divergent on `number`, else Exact |
| `x [NOT] IN (…)` | same | Divergent on `number`, else Exact |
| `s [NOT] LIKE p` | `s [NOT] LIKE p` | Exact |
| `s SIMILAR TO p` | — | **Unsupported** |
| `CASE WHEN … END` | `CASE WHEN … END` | Exact |
| `LENGTH(s)` | `CHAR_LENGTH(s)` | Exact |
| `LOWER(s)`, `UPPER(s)`, `TRIM(s)` | `LOWER(s)`, `UPPER(s)`, `TRIM(s)` | Exact |
| `STARTS_WITH(s, 'NZ-')` | `s LIKE 'NZ-%' ESCAPE '\'`, the prefix escaped | Guarded |
| `ENDS_WITH(s, '.nz')` | `s LIKE '%.nz' ESCAPE '\'`, the suffix escaped | Guarded |
| `ABS(x)`, `FLOOR(x)` | `ABS(x)`, `FLOOR(x)` | Exact |
| `CEIL(x)` | `CEILING(x)` | Exact |
| `ROUND(x)` | `ROUND(CAST(x AS NUMERIC))` | Guarded |
| `ROUND(x, d)` | `ROUND(CAST(x AS NUMERIC), d)` | Guarded |
| `MOD(x, y)` | `MOD(MOD(x, y) + y, y)` | Divergent |
| `NOW()` | `CURRENT_TIMESTAMP` | Exact |
| `interval(n, unit)` | `INTERVAL 'n unit'` | Exact |
| `MIN(x)`, `MAX(x)`, `SUM(x)`, `AVG(x)` | same | Exact |
| `COUNT(x)` | `COUNT(x)` | Exact |
| `ROW_COUNT()` | `COUNT(*)` | Exact |
| `COUNT_DISTINCT(x)` | `COUNT(DISTINCT x)` | Exact |
| `ANY(b)` | `CASE WHEN COUNT(b) = 0 THEN NULL ELSE MAX(CASE WHEN b THEN 1 ELSE 0 END) = 1 END` | Guarded |
| `ALL(b)` | as `ANY`, with `MIN` | Guarded |
| `IS_INFINITE(x)` | `x = CAST('Infinity' AS DOUBLE PRECISION) OR x = CAST('-Infinity' AS DOUBLE PRECISION)` | Guarded |
| `IS_NAN(x)`, `IS_FINITE(x)` | — | **Unsupported** |
| a number literal | `42`, `3.14` — integers without a point | Exact |
| `INF`, `NAN` | `CAST('Infinity' AS DOUBLE PRECISION)`, `CAST('NaN' AS DOUBLE PRECISION)` | Exact |
| a string literal | `'…'`, single quotes doubled | Exact |
| a date/datetime literal | `DATE '2000-01-01'`, `TIMESTAMP '…'` | Exact |
| `NULL`, `TRUE`, `FALSE` | `NULL`, `TRUE`, `FALSE` | Exact |
| `COLUMNS(...)` | the expanded conjunction | Exact |

: {tbl-colwidths="[32,50,18]"}

### Refusals

| Construct | Why |
|-----------|-----|
| `SIMILAR TO` | Standard SQL has an operator spelled `SIMILAR TO`, but it matches a different pattern language than [the language's RE2](expressions.md#pattern-matching), and DuckDB and PostgreSQL spell regex matching differently (`regexp_matches` / `~`). Emitting the standard operator would be a mistranslation, so the target refuses instead. Use `SQL(duckdb)` or `SQL(postgres)`. |
| `LIKE` with a computed pattern | The `ESCAPE`-based prefix and suffix translations require the pattern to be a literal so it can be escaped at translation time. A literal pattern is the ordinary case. |
| `IS_NAN`, `IS_FINITE` | Detecting a NaN needs `isnan` in DuckDB and `x = 'NaN'` in PostgreSQL, and the standard `x <> x` idiom works on neither, because both engines make a NaN equal to itself. There is no portable spelling, so the target refuses rather than emitting one that is wrong on one engine. Use `SQL(duckdb)` or `SQL(postgres)`. |

: {tbl-colwidths="[28,72]"}

### Why these spellings

Each guard below exists because the two engines disagree, or because both disagree with the language.

**Integer division.** `1 / 2` is `0` in PostgreSQL and `0.5` in DuckDB, and [the language says `0.5`](floating-point.md#integers-and-floats). Casting one operand makes both engines agree with the language.

**Zero divisors are left bare.** [The language gives an infinity or a NaN](floating-point.md#non-finite), and so does DuckDB — `7/0` is `inf`, `0/0` is `nan` — so the plain spelling is exactly right there. PostgreSQL is the odd one out: it raises `division_by_zero`, and raises on `MOD(7, 0)` too, where DuckDB gives null for an integer modulus. No portable expression gives an infinity, so this can't be guarded, only declared. It is one of the two places `SQL(ANSI)` output means different things on the two engines, and the reason `/` and `MOD` are Divergent rather than Exact.

**Comparison against a NaN can't be guarded.** [The language says](floating-point.md#non-finite) `NAN = NAN` is `false` and `NAN > 1` is `false`. Both engines instead sort a NaN above every number and make it equal to itself, so both say `true` where the language says `false`. There is no portable fix. The usual way to spot a NaN is `x <> x`, and that is exactly what these engines break: if a NaN equals itself, `x <> x` is `false` for a NaN too. Testing for one needs `isnan` in DuckDB and `x = 'NaN'` in PostgreSQL, so the comparison operators are Divergent wherever a `number` can reach them. That is the second place `SQL(ANSI)` means different things on the two engines, and the reason `IS_NAN` and `IS_FINITE` are refused. `IS_INFINITE` is fine, because comparing against an infinity works the same on both.

**A floored modulo needs a guard.** [The language takes `MOD`'s sign from the divisor](expressions.md#mod), as R and Python do; the SQL standard, PostgreSQL and DuckDB all take it from the dividend. `MOD(MOD(x, y) + y, y)` corrects the sign on both engines, and keeps an integer result an integer — the `x - y * FLOOR(x / y)` form would not, since the language's `/` is float division. The zero divisor is why this mapping is Divergent rather than Guarded: the language gives a NaN, PostgreSQL raises, and DuckDB gives null for an integer modulus.

**Shifting a date needs no cast.** `date + interval` produces a timestamp in both engines, and [so does the language](expressions.md#arithmetic) — the rule was chosen to match them. Nothing to guard.

**`ROUND` through `NUMERIC`.** On floats the engines disagree outright: PostgreSQL rounds halves to even (`ROUND(0.5)` is `0`) and DuckDB away from zero (`1`), and PostgreSQL has no two-argument `ROUND` for floats at all. Casting to `NUMERIC` first makes both round halves away from zero, matching [the language](expressions.md#round) exactly, negative `digits` included. So `SQL(ANSI)` avoids the [rounding divergence](#standing-divergences) the dataframe targets have to live with.

**`CHAR_LENGTH`.** `LENGTH` counts characters in some engines and bytes in others; `CHAR_LENGTH` is unambiguous.

**`ANY`/`ALL`.** `BOOL_OR`/`BOOL_AND` are not standard, and the standard's `ANY`/`ALL` are subquery quantifiers rather than aggregates, so the fold is written with `CASE`. A bare `MAX(CASE …)` fold would return `false` on all-null input where [the language returns null](expressions.md#empty-input), so it is wrapped in a `COUNT` test. That makes `SQL(ANSI)` exact on the empty-aggregate case too.

**`CURRENT_TIMESTAMP`.** Written without parentheses, which the standard requires and both engines accept.

**One thing the target cannot guard.** `SUM` over integers widens to 128 bits in DuckDB, so a sum that the reference implementation reports as an [overflow](#no-result) may quietly succeed there. Narrowing it would cost more than it is worth, and the direction of the disagreement is benign — the validator is the strict one.
