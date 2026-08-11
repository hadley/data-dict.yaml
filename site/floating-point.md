# Floating point

There is one `number` [type](expressions.md#types), but underneath it a number is held one of two ways, and the difference decides how exact the arithmetic is and what happens at the edges. This page collects those rules in one place: what makes a number an integer or a float, what `INF` and a NaN mean, how both behave when an assertion is [evaluated](expression-execution.md), and where the [languages it translates to](expression-execution.md#translating-expressions) disagree.

## Integers and floats {#integers-and-floats}

No expression is ever ill-typed for mixing whole numbers with fractional ones. Underneath, though, a number is held as an **integer** or as a **float**:

* A literal without a decimal point is an integer (`42`); one with a decimal point is a float (`42.0`, `3.14`). A whole-numbered column read from the data is an integer, a fractional one a float.
* `+`, `-`, `*` and `MOD` over two integers give an integer. Every other combination gives a float. `MOD(x, 0)` is the one exception: it is [a NaN](#non-finite), and a NaN is a float.
* `/` **always** gives a float, so `1 / 2` is `0.5`. This is the one place the two representations would otherwise disagree about the answer, and it follows R, Python and DuckDB rather than SQL's integer division.
* Of the aggregates, `SUM` of integers is an integer and `AVG` is always a float, as their own entries say; `COUNT`, `COUNT_DISTINCT` and `ROW_COUNT` are integers.

Integers are 64-bit and exact. Arithmetic that overflows that range is not silently wrapped or rounded — it is reported when the data is validated, as [D09](validation.md#data-validation-checks). Floats are 64-bit IEEE 754 and carry all the usual caveats, so an equality test on a computed float (`price * qty = total`) is rarely what you want; compare a rounded value, or bound the difference.

The distinction matters mostly to [translation](expression-execution.md#translating-expressions): R, Python and DuckDB divide the same way, but PostgreSQL and standard SQL divide two integers into an integer, so `1 / 2` is `0` there unless the translation casts first.

## Infinity and NaN {#non-finite}

A float can also be positive infinity, negative infinity, or a NaN ("not a number"). These are values like any other. They are not null, they are not errors, and an expression that produces one carries on with it.

They arrive two ways. **Arithmetic produces them**, following IEEE 754: `7 / 0` is `INF`, `-7 / 0` is `-INF`, `0 / 0` is a NaN, and `MOD(x, 0)` is a NaN. **Data contains them**, since a float column may hold any of the three, and `validate-data` reads what is there rather than rejecting it.

They are written `INF` and `NAN`. `-INF` is [unary minus](expressions.md#operators) applied to `INF`, exactly as a leading `-` works on any other number. All three are `number`s and go wherever a number goes.

Three predicates ask which kind of number a value is:

| Function         | Signature          | True when                  |
|------------------|--------------------|----------------------------|
| `IS_FINITE(x)`   | `number → boolean` | `x` is an ordinary number. |
| `IS_INFINITE(x)` | `number → boolean` | `x` is `INF` or `-INF`.    |
| `IS_NAN(x)`      | `number → boolean` | `x` is a NaN.              |

: {tbl-colwidths="[24,26,50]"}

All three are ordinary scalar functions, and so [null-propagating](expressions.md#operators): `IS_NAN(NULL)` is null, not `false`. Asking whether a value is *missing* remains `IS NULL`'s job, and `IS NULL` is `false` for a NaN — a NaN is present, it is simply not a number.

## Comparison {#comparison}

**Comparison follows IEEE 754.** A NaN is unordered, so every comparison against one is `false` except `<>`, which is `true`: `NAN = NAN` is `false`, `NAN > 1` is `false`, `NAN < 1` is `false`, and `NAN <> NAN` is `true`. The infinities compare normally — `-INF < x < INF` for every finite `x`, and `INF = INF` is `true`. `BETWEEN` and `IN` inherit these answers, since [both are defined by comparison](expressions.md#membership).

Answering `false` rather than null is deliberate, and it is why a zero divisor needs no diagnostic. Under [`CHECK` semantics](expressions.md#truth-and-null) null passes, so a null answer would quietly retire a rule on exactly the rows a NaN makes suspect; `false` reports them. **An assertion is violated by a NaN wherever it compares one.** `total / qty > 1` is `false` on a row where `total` and `qty` are both `0`, and that row is reported. Write `IS_NAN(total / qty) OR total / qty > 1` to tolerate it, or `qty <> 0 AND total / qty > 1` to exclude the zero itself.

Two spellings that are otherwise interchangeable stop being so here. `NOT (x < y)` and `x >= y` differ when either side is a NaN: the first is `true`, the second `false`. Where a column can hold one, prefer the positively-stated form, or conjoin `IS_FINITE(...)`.

### Equality is not identity {#identity}

`=` is IEEE, so no NaN equals any NaN. But several places have to decide whether two values are *the same value* rather than whether they compare equal, and there the language uses one **identity order** instead: values run `-INF` < every finite number < `INF` < NaN, all NaNs count as one value, and `-0.0` and `+0.0` count as one value.

The identity order governs `MIN`, `MAX` and `COUNT_DISTINCT` in an expression, and the `unique`/`primary_key` ([D02](validation.md#data-validation-checks)) and `foreign_key` ([D05](validation.md#data-validation-checks)) checks at [the data level](validation.md#comparable-types). So `COUNT_DISTINCT` of a column of nothing but NaNs is 1; `MIN` of a column containing a NaN is its smallest ordinary value, and `MAX` is the NaN.

`SUM` and `AVG` are arithmetic rather than ordering, so they follow IEEE and propagate: one NaN anywhere in the column makes both a NaN, and an `INF` and a `-INF` in the same column sum to a NaN. `COUNT` counts a NaN, because a NaN is not null.

## Evaluation {#evaluation}

Division by zero is an infinity or a NaN: `7 / 0` is `INF`, `0 / 0` is a NaN, `MOD(x, 0)` is a NaN. Nothing is reported, and evaluation carries on.

This is safe because comparing anything against a NaN gives `false`, so the row is reported as a violation. Giving null instead would be unsafe, because [null passes](expression-execution.md#what-counts-as-a-violation): `total / qty > 1` would then go unchecked on exactly the rows whose `qty` is most suspect.

Integer overflow is a violation, and it is [reported as D09](expression-execution.md#no-result). Floats are unaffected: they overflow to `INF`, which is a value, so the expression still reaches a verdict.

The profile treats the same values differently, on purpose: it counts them [separately](export.md#profile) instead of binning them. A profile shows where values sit on the number line, and an infinity has no place there — it would stretch every bin. An assertion just asks whether a rule holds, and there an infinity is an ordinary value.

## In a dictionary {#in-a-dictionary}

A `range` bound may be `-.inf` or `.inf` to [leave that end open](spec.md#representative-values). That is a statement about the bound, not about the data: it says the true extent is unknown or moving, rather than that the column was observed to hold an infinity. `.nan` is not a bound at all, and is no more an `examples` value than a bound: both are rejected (S12).

## Across languages {#across-languages}

R, Python and SQL disagree over what `x / 0` gives, over whether a NaN equals itself, and over whether a NaN counts as missing. The first of those is [a standing divergence](expression-execution.md#standing-divergences) of [translation](expression-execution.md#translating-expressions); the other two are the two halves of what a NaN *means*, and the different languages answer both three different ways.

| Language | `NaN = NaN`, `NaN > 1` | `NaN IS NULL` | an aggregate over a NaN |
|----------|------------------------|---------------|-------------------------|
| data-dict | `false` | `false` | folded in |
| `SQL(duckdb)`, `Python(polars)` | `true` | `false` | folded in |
| `SQL(postgres)` | `true` | `false` | folded in |
| `R(*)` | `NA` | `TRUE` | dropped by `na.rm` |
| `Python(pandas)`, Arrow-backed | `NA` | `True` | dropped |
| `Python(pandas)`, NumPy-backed | `False` | `True` | dropped |

: {tbl-colwidths="[30,24,16,30]"}

Every disagreement goes the same way: the translation says `true` or null where data-dict says `false`, and both of those [pass](expression-execution.md#what-counts-as-a-violation). So on a row holding a NaN a translated rule is more forgiving than this one, never stricter. A language that can recover data-dict's answer with a short guard gets one; one that can't says so in a note. Either way, a dictionary whose float columns hold NaNs should be checked with `validate-data`.

Both pandas backends treat a NaN as *missing*  (`isna` is `True` and every aggregate drops it) where [the language treats it as a value](#non-finite). The two backends then disagree with each other about comparison: NumPy-backed pandas says `False`, which is the language's answer, and Arrow-backed pandas says `NA`, which isn't. pandas translations still assume nullable ("Arrow-backed") dtypes, because that backend's `&` and `|` follow the same three-valued logic as the language — and every rule uses that logic, while only a column that actually holds a NaN uses NaN comparison. Both assumptions travel as notes rather than as guard code; guarding every comparison and every aggregate for pandas is not attempted.
