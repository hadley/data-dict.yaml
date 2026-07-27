# Expressions

data-dict provides a small expression language: a SQL-like sublanguage for stating a rule about the values in a single row of a single table. You write one wherever a dictionary needs to say something a keyword can't, which today means the `assert` key of a column or table [constraint](spec.md#column-constraints):

```yaml
tables:
  - name: survey
    constraints:
      - assert: end_date >= start_date
        description: A contract can't end before it starts.
    columns:
      - name: postcode
        type: string
        constraints:
          - required
          - assert: LENGTH(postcode) <= 10
```

An expression is always written against the columns of one table: bare names are that table's column names, and an expression on a column sees every other column too. So the two constraints above differ in where they're written, not in what they can say — a column constraint sits next to the column it's mostly about, and a table constraint is the natural home for a rule that spans columns.

This page is the reference for the language. It covers [how an expression is evaluated](#evaluation), [truth and null](#truth-and-null), the [types](#types) values carry, the [literals](#literals), [operators](#operators), and [functions](#functions) available, [`COLUMNS(...)`](#selecting-multiple-columns) for applying one predicate to many columns, the [type rules](#type-checking) a validator enforces, and the [grammar](#grammar).

## Evaluation

**One row of one table.** An expression sees the columns of a single row at a time, and evaluating it against every row in turn is the whole of its meaning. There are no aggregates and no subqueries — cross-table rules belong in [`relationships`](spec.md#relationships), and the per-row restriction keeps expressions cheap to check.

**Deterministic, apart from `NOW()`.** The same row always gives the same result. `NOW()` is the one exception: it reads the current time, so a freshness check like `observed_at >= NOW() - interval(2, weeks)` can pass one day and fail the next even though the data hasn't changed.

**Case sensitivity.** Keywords and function names are case-insensitive, as in SQL — `AND`, `and`, `LENGTH`, and `length` are all accepted. Column names are the exception: unlike SQL's unquoted identifiers, they are case-sensitive, matched exactly against the column names in the data (which may themselves differ only by case), consistent with how `name` is matched everywhere else. Value comparisons (`=`, `LIKE`, `SIMILAR TO`, …) are case-sensitive too.

## Truth and null

Expressions use SQL's three-valued logic, so for a given row an expression is `true`, `false`, or `null` (unknown). A comparison involving a null operand is `null`, not `false` — `LENGTH(postcode) <= 10` is `null` when `postcode` is null.

An expression used as an assertion follows SQL's `CHECK` semantics: the row **passes** when the expression is `true` **or** `null`, and only a `false` result is a violation. An assertion therefore never doubles as a null check. `LENGTH(postcode) <= 10` constrains the values that *are* present but says nothing about missing ones; pair it with the `required` constraint (or an explicit `IS NOT NULL`) when the column must also be non-null.

Because an assertion states what must be **true**, conditional rules are written as implications: "if `q3` then `q4`" becomes `NOT(q3) OR q4 IS NOT NULL`, or equivalently `NOT(q3 AND q4 IS NULL)`.

## Types

Every expression has a type. These are the language's own types, close to but not identical with the column [types](spec.md#types) in the dictionary:

| Type | Where it comes from |
|------|---------------------|
| `number` | A `number` column (any measure), a number-valued `enum`, a numeric literal, arithmetic, or a numeric function. |
| `string` | A `string` column, a string-valued `enum`, or a quoted literal. |
| `boolean` | A `boolean` column, a boolean-valued `enum`, `TRUE`/`FALSE`, or any comparison or logical operator. |
| `date` | A `date` column, or a date plus or minus an interval. |
| `datetime` | A `datetime` column, `NOW()`, or a datetime plus or minus an interval. |
| `interval` | `interval(<n>, <unit>)`. A duration; it only exists inside an expression, never as a column type. |

: {tbl-colwidths="[15,85]"}

An **`enum`** takes the type of its `values` since that's what the column actually holds: `[M, F, U]` is a `string`, `[1, 2, 3]` a `number`, and `[true, false]` a `boolean`. The map form takes the type of its keys, so `{M: Male, F: Female}` is a `string` too. An enum behaves exactly like its value type — a string enum can be measured with `LENGTH` and matched with `LIKE`, a number enum can be compared with `<`.

A column listed by **name only**, with no `type`, has an unknown type. Using one where a type is needed is an error (S23): nothing can be checked about such an expression, and the fix is something the dictionary wants anyway — declare the column's `type`.

An unknown type is only a problem where a type is actually needed. `IS NULL` and `IS NOT NULL` ask nothing of their operand, so `u IS NOT NULL` — and `COLUMNS(*) IS NOT NULL` on a table with undocumented columns — is fine; `LENGTH(u)` and `u > 5` are not.

`NULL` is the one genuinely typeless thing in the language, and stays compatible with every type.

The `number` measures (`number(id)`, `number(ordinal)`, `number(quantity)`) and a `datetime`'s `time_zone` do not affect type checking: all three measures are just `number`, and a `datetime` is a `datetime` whatever zone it declares. Nor does an enum's type follow the *look* of its values: `values: [2024-01-01, 2024-01-02]` is a `string` enum, not a `date` column, so it can't be compared with `NOW()`.

## Literals

| Form | Type | Examples |
|------|------|----------|
| Integer or decimal | `number` | `42`, `3.14`. A leading `-` is unary minus, not part of the literal. |
| Single-quoted text | `string` | `'ABC'`, `'O''Brien'` — double a quote to include one. |
| `TRUE` / `FALSE` | `boolean` | |
| `NULL` | none | Compatible with every type. |

: {tbl-colwidths="[25,15,60]"}

There is no dedicated date or datetime literal. Write a date or datetime as a single-quoted ISO 8601 string and compare it against a `date`/`datetime` column — `birthdate >= '2000-01-01'`, `observed_at < '2024-01-31T09:30:00Z'`. Such a literal is a plain string until it meets a temporal column; see [comparisons](#comparability). To *compute* a datetime rather than write one down, use `NOW()` and `interval(...)`.

## Operators

Below, `T` stands for any type, and a signature like `number → number` reads "takes a number, returns a number". Operators are **null-propagating**: if any operand is null the result is null. Three groups are exempt, and they're the ones that make three-valued logic usable:

* `IS NULL` and `IS NOT NULL` inspect nullness, so they always return `true` or `false`.
* `AND` and `OR` short-circuit on a decisive operand: `false AND NULL` is `false`, and `true OR NULL` is `true`.
* `CASE` returns whichever branch its conditions select, null or not.

### Arithmetic

| Operator | Signature | Notes |
|----------|-----------|-------|
| `-x` | `number → number` | Unary minus. |
| `x + y`, `x - y` | `number, number → number` | |
| `x * y`, `x / y` | `number, number → number` | Division by zero yields null. |
| `d + i`, `i + d`, `d - i` | `date, interval → date` | Shifts a date by a duration. |
| `t + i`, `i + t`, `t - i` | `datetime, interval → datetime` | Shifts a datetime by a duration. |

: {tbl-colwidths="[20,32,48]"}

Interval arithmetic is the only non-numeric arithmetic: a date or datetime plus or minus an interval keeps its own type. Subtracting one date from another is not supported, nor is multiplying an interval.

### Comparison

| Operator | Signature | Notes |
|----------|-----------|-------|
| `x = y` | `T, T → boolean` | |
| `x != y`, `x <> y` | `T, T → boolean` | The two spellings are identical. |
| `x < y`, `x <= y`, `x > y`, `x >= y` | `T, T → boolean` | Ordered types: numbers, strings, dates, datetimes. |

: {tbl-colwidths="[28,25,47]"}

Both sides must be [comparable](#comparability). String comparison is by code point, and so case-sensitive.

### Null tests

| Operator | Signature | Notes |
|----------|-----------|-------|
| `x IS NULL` | `T → boolean` | |
| `x IS NOT NULL` | `T → boolean` | |

: {tbl-colwidths="[25,22,53]"}

### Logic

| Operator | Signature | Notes |
|----------|-----------|-------|
| `NOT x` | `boolean → boolean` | `NOT NULL` is null. |
| `x AND y` | `boolean, boolean → boolean` | Null unless one side is `false`, which makes the whole thing `false`. |
| `x OR y` | `boolean, boolean → boolean` | Null unless one side is `true`, which makes the whole thing `true`. |
| `( … )` | grouping | |

: {tbl-colwidths="[20,30,50]"}

### Membership

| Operator | Signature | Notes |
|----------|-----------|-------|
| `x BETWEEN lo AND hi` | `T, T, T → boolean` | Inclusive on both ends; equivalent to `x >= lo AND x <= hi`. |
| `x NOT BETWEEN lo AND hi` | `T, T, T → boolean` | |
| `x IN (a, b, …)` | `T, T… → boolean` | Each list item must be comparable with `x`. |
| `x NOT IN (a, b, …)` | `T, T… → boolean` | Null if `x` is null or `x` matches nothing but the list contains a null. |

: {tbl-colwidths="[30,25,45]"}

### Pattern matching

| Operator | Signature | Notes |
|----------|-----------|-------|
| `s LIKE p` | `string, string → boolean` | `%` matches any run of characters, `_` any single character. |
| `s NOT LIKE p` | `string, string → boolean` | |
| `s SIMILAR TO p` | `string, string → boolean` | `p` is a regular expression. |
| `s NOT SIMILAR TO p` | `string, string → boolean` | |

: {tbl-colwidths="[28,27,45]"}

A `SIMILAR TO` pattern is an [RE2](https://github.com/google/re2/wiki/Syntax) regular expression, anchored so it must match the whole string (as in DuckDB, and unlike PostgreSQL's `SIMILAR TO`, it does not use `%`/`_` wildcards). Both operators are case-sensitive; use `LOWER` on the subject, or an inline `(?i)` flag in a `SIMILAR TO` pattern, for a case-insensitive match.

### Conditional

`CASE WHEN c1 THEN r1 [WHEN c2 THEN r2 …] [ELSE r] END` takes one or more `boolean` conditions, and returns the result of the first whose condition is `true`. If none is, it returns the `ELSE` result, or null when there is no `ELSE`. The `WHEN` conditions must be boolean; the results may be any type, but keep them all the same type so the `CASE` itself has one (see [type checking](#type-checking)).

```yaml
- assert: CASE WHEN kind = 'weight' THEN value < 1000 ELSE value < 10 END
```

Only the searched form (`CASE WHEN <condition>`) is supported, not the simple form (`CASE <expr> WHEN <value>`). An implication written with `NOT`/`OR` is usually shorter and clearer than the equivalent `CASE`.

### Precedence

From tightest to loosest, following standard SQL:

1. `( )`, function calls, `CASE`
2. unary `-`
3. `*`, `/`
4. `+`, `-`
5. comparisons, `IS NULL`, `BETWEEN`, `IN`, `LIKE`, `SIMILAR TO`
6. `NOT`
7. `AND`
8. `OR`

Binary operators of equal precedence associate left to right. Comparisons don't chain, though — `lo < x < hi` is a syntax error; write `x BETWEEN lo AND hi`, or `x > lo AND x < hi` if you want the bounds exclusive.

Use parentheses when in doubt: `NOT(q3) OR q4 IS NOT NULL` and `NOT (q3 OR q4 IS NOT NULL)` are different rules.

## Functions

Function names are case-insensitive. Every function is null-propagating: any null argument gives a null result. Passing the wrong number of arguments, or an argument of the wrong type, is an error at spec-validation time, not something that happens per row.

### String functions

#### `LENGTH(s)`

`string → number`. The number of characters (Unicode code points) in `s`, not the number of bytes.

```yaml
- assert: LENGTH(postcode) <= 10
```

#### `LOWER(s)`, `UPPER(s)`

`string → string`. `s` with each character mapped to lower or upper case. Useful for making a comparison case-insensitive:

```yaml
- assert: LOWER(country) IN ('nz', 'au')
```

#### `TRIM(s)`

`string → string`. `s` with leading and trailing whitespace removed. Only the one-argument form is supported — there's no custom trim set, and no separate `LTRIM`/`RTRIM`.

```yaml
- assert: TRIM(name) = name
  description: Names are stored without surrounding whitespace.
```

#### `STARTS_WITH(s, prefix)`, `ENDS_WITH(s, suffix)`

`string, string → boolean`. Whether `s` begins or ends with the given substring. Both arguments are strings and the match is case-sensitive and literal — no wildcards. For a pattern, use `LIKE` or `SIMILAR TO` instead.

```yaml
- assert: STARTS_WITH(sku, 'NZ-')
```

### Numeric functions

#### `ABS(x)`

`number → number`. The absolute value of `x`.

#### `FLOOR(x)`, `CEIL(x)`

`number → number`. `x` rounded down (towards negative infinity) or up (towards positive infinity) to a whole number.

#### `ROUND(x)`, `ROUND(x, digits)`

`number → number` or `number, number → number`. `x` rounded to `digits` decimal places, or to a whole number when `digits` is omitted. Halves round away from zero, so `ROUND(0.5)` is `1` and `ROUND(-0.5)` is `-1`. A negative `digits` rounds to the left of the decimal point (`ROUND(1234, -2)` is `1200`). This is the only function whose arity varies.

```yaml
- assert: ROUND(share, 2) = share
  description: Shares are recorded to two decimal places.
```

#### `MOD(x, y)`

`number, number → number`. The remainder of `x / y`, taking its sign from `x` (so `MOD(-7, 3)` is `-1`). Null when `y` is zero.

```yaml
- assert: MOD(minutes, 15) = 0
  description: Appointments start on a quarter hour.
```

### Date and time functions

#### `NOW()`

`→ datetime`. The current time, as an instant. Takes no arguments, and is the one non-deterministic thing in the language: an expression that uses it depends on when it runs. Its value is fixed for the whole evaluation, so two `NOW()`s in one expression always agree.

#### `interval(n, unit)`

`number, unit → interval`. A duration of `n` units, for use with `+` or `-` on a `date` or `datetime`. `n` is a number and `unit` is a bare keyword (not a string), one of:

| Unit | Length |
|------|--------|
| `seconds` | 1 second |
| `minutes` | 60 seconds |
| `hours` | 60 minutes |
| `days` | 24 hours |
| `weeks` | 7 days |

: {tbl-colwidths="[20,80]"}

All five are fixed-length. Calendar units (`months`, `years`) are deliberately excluded: they're non-uniform (adding a month lands on a different number of days depending on the date, and clamps at month ends), which would make an expression's meaning depend on the calendar. Express a rough month as `interval(30, days)` if you need it.

```yaml
- assert: observed_at >= NOW() - interval(2, weeks)
  description: The export always covers the last fortnight.
```

## Selecting multiple columns

To apply the same predicate to a group of columns without repeating it, use a `COLUMNS(...)` expression — a simple subset of [DuckDB's `COLUMNS`](https://duckdb.org/docs/current/sql/expressions/star). The supported forms select columns by:

| Form | Selects |
|------|---------|
| `COLUMNS(*)` | Every column in the table. |
| `COLUMNS('<regex>')` | Every column whose name matches the regular expression. |
| `COLUMNS([a, b, c])` | An explicit list of column names. |

: {tbl-colwidths="[30,70]"}

The regex is an [RE2](https://github.com/google/re2/wiki/Syntax) expression matched **unanchored** (a partial match, as in DuckDB), so `COLUMNS('q')` selects every column whose name contains a `q`; anchor it with `^`/`$` to match the whole name. This is the one place a regex is unanchored — `SIMILAR TO` anchors.

A `COLUMNS(...)` node stands in for a column reference, and the expression around it is evaluated once per selected column. The result is true only when it is true for **every** selected column — the per-column results are combined with `AND`.

```yaml
constraints:
  # Every q4–q8 answer is present whenever q3 is true.
  - assert: NOT(q3) OR COLUMNS('q[4-8]') IS NOT NULL
    description: q4–q8 must be answered when q3 is true.
  # No column anywhere in the table is null.
  - assert: COLUMNS(*) IS NOT NULL
```

Three rules bound the feature:

* **At most one `COLUMNS(...)`** may appear in an expression, so there's no ambiguity about how two selections would combine.
* **Every selected column must fit the way the expression uses it**, since the predicate is applied to each in turn. `LENGTH(COLUMNS('name_.*'))` requires that each matched column is a string, just as a bare column reference would.
* **A regex that matches nothing is a warning** (S22). It's almost always a typo, and the expression would otherwise hold vacuously. `COLUMNS(*)` and an explicit list can't trigger it — an empty list isn't expressible, and an unknown name in a list is an error (S20) rather than a warning.

The lambda form (`COLUMNS(c -> ...)`) and the star modifiers (`EXCLUDE`, `REPLACE`, `RENAME`) are **not** supported.

## Type checking

Expressions are checked when the dictionary is validated, against the columns of the enclosing table alone — before any data is read. A malformed expression is S19, an unknown column S20, an ill-typed expression S21, and an empty column selection S22; see [validation](validation.md) for the full list.

Four rules decide whether an expression is well typed, and a fifth (S23, above) requires that every operand whose type matters has one. The more the dictionary says about a column, the more of an expression can be checked.

### The expression as a whole must be boolean

An expression states a rule, so its result must be a truth value. A bare non-boolean column is not a rule on its own; compare or test it (`qty > 0`, `postcode IS NOT NULL`). A bare top-level `COLUMNS(...)` is fine so long as every column it selects is boolean.

### Operands must match their operator or function

The signatures above are enforced exactly: `LENGTH(qty)` on a numeric column is an error, as is `LENGTH('a', 'b')`, as is calling a function that doesn't exist.

### Compared values must be comparable {#comparability}

Two operands may be compared when any of the following holds:

* They have the same type.
* Both are temporal — a `date` may be compared with a `datetime`, so `some_date < NOW()` is fine.
* One is a string literal whose text parses as the other side's type, which is how `birthdate >= '2000-01-01'` works. This applies to literals only; a `string` *column* can't be compared with a `date` column.
* One is `NULL`, which is compatible with everything.

### A `CASE` must have one result type

Its type is the common type of its branches; branches of differing types make the whole `CASE` typeless, which then passes any comparison it's used in. That's permitted but rarely what you want.

## Grammar

For reference, the full grammar, loosest to tightest:

```text
expr           := or_expr
or_expr        := and_expr ("OR" and_expr)*
and_expr       := not_expr ("AND" not_expr)*
not_expr       := "NOT" not_expr | predicate
predicate      := additive ( cmp additive
                           | "IS" ["NOT"] "NULL"
                           | ["NOT"] "BETWEEN" additive "AND" additive
                           | ["NOT"] "IN" "(" expr ("," expr)* ")"
                           | ["NOT"] "LIKE" additive
                           | ["NOT"] "SIMILAR" "TO" additive )?
additive       := multiplicative (("+" | "-") multiplicative)*
multiplicative := unary (("*" | "/") unary)*
unary          := "-" unary | primary
primary        := literal | column | funcall | columns | case | "(" expr ")"
cmp            := "=" | "!=" | "<>" | "<" | "<=" | ">" | ">="
literal        := number | string | "TRUE" | "FALSE" | "NULL"
funcall        := IDENT "(" (expr ("," expr)*)? ")"   // incl. NOW(), interval(n, unit)
columns        := "COLUMNS" "(" ("*" | string | "[" IDENT ("," IDENT)* "]") ")"
case           := "CASE" ("WHEN" expr "THEN" expr)+ ("ELSE" expr)? "END"
IDENT          := [A-Za-z_][A-Za-z0-9_]*
```

The following words are reserved and can't be used as a bare column name: `AND`, `OR`, `NOT`, `IS`, `NULL`, `BETWEEN`, `IN`, `LIKE`, `SIMILAR`, `TO`, `WHEN`, `THEN`, `ELSE`, `END`, `TRUE`, `FALSE`.
