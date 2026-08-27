# datadict

<!-- badges: start -->
<!-- badges: end -->

datadict validates a dataset against a [data-dict.yaml](https://data-dict.tidyverse.org)
data dictionary, and reports the findings as a self-contained HTML report.

## Installation

``` r
# Install the released version from CRAN
install.packages("testthat")

# Or the development version from GitHub:
# install.packages("pak")
pak::pak("r-lib/testthat")
```

## Example

Point `dd_validate_data()` at a directory holding a `data-dict.yaml` file and
the parquet files it describes:

``` r
library(datadict)

dd_validate_data("inst/data")
```

This writes an HTML report and opens it in a browser. A dataset that fails
validation is not an R error: the report is the point, so check the returned
`status` to see whether the run passed.
