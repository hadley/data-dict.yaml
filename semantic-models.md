# Why not a semantic model?

Tools like Looker, dbt, and other BI platforms use semantic models that classify columns as either **dimensions** (things you group or filter by) or **metrics** (things you aggregate). `dd.yaml` deliberately avoids this split. Here's why.

## The dimension/metric split is about intent

The distinction between a dimension and a metric is really about how you plan to use it, not an intrinsic property of the columns. Kimball's framework assumes you've decided in advance how each field will be used. This works works well for structured reporting but gets strained when analysts want to explore data more freely.

For example, a product rating (1--5) could be a dimension if you're filtering ("show returns for 1-star products") or a metric if you're aggregating ("average rating by category"). Encoding one intended use in the data dictionary constrains the other.

## Dimension groups solve a problem data scientists don't have

Looker's dimension groups automatically generate derived columns like `created_date`, `created_week`, and `created_month`, so analysts can group by whatever granularity they need without the model author defining each one manually. This is valuable in a BI context, but data scientists already have the tools to do this themselves. They're also likely to want to join with external data (like holidays or fiscal calendars) that a dimension group can't anticipate.

## BI tools and data science have different constraints

BI tools need semantic models because the end user can't create their own columns --- every useful view of the data has to be predefined. Data scientists face a different challenge: they can derive any column they need, but they need to understand what the raw data *means*. That's the problem `dd.yaml` is designed to solve.
