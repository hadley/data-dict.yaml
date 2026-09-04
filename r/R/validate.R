#' Validate a dataset's values against its data dictionary
#'
#' Runs `data-dict render-report`, which validates the data and writes the
#' findings as a self-contained HTML report, and opens it in a browser. The
#' data files themselves come from each
#' table's `source: parquet:` entry in the dictionary, resolved relative to the
#' dictionary's directory.
#'
#' A dataset that fails validation is not an R error: the report is the point,
#' so check `status` to see whether the run passed.
#'
#' @param dict A `data-dict.yaml` file, or a directory holding one.
#' @param table Validate only this table, instead of every table in the
#'   dictionary.
#' @param html Where to write the report.
#' @param browse Whether to open the report with [utils::browseURL()].
#' @return A list with the `html` path and the CLI's exit `status`, where `0`
#'   means the dataset validated, invisibly.
#' @export
#' @examplesIf FALSE
#' # a directory holding a data-dict.yaml and the parquet files it points at
#' dd_validate_data("inst/data")
dd_validate_data <- function(dict = ".",
                             table = NULL,
                             html = tempfile(fileext = ".html"),
                             browse = interactive()) {
  args <- c("render-report", dict)
  if (!is.null(table)) {
    args <- c(args, "--table", table)
  }
  args <- c(args, "--output", html)

  # The CLI would overwrite `html` anyway; clearing it first means a stale
  # report can't be mistaken for the one this run failed to write.
  unlink(html)
  # Not dd_run(): a dataset that fails validation exits non-zero too, and the
  # CLI has no exit code that tells the two apart.
  run <- run_binary(args)
  status <- run$status

  # The CLI writes no page when the run could not be started at all.
  if (!file.exists(html)) {
    stop(
      run_failure(
        paste0("data-dict wrote no report (exit status ", status, "):"),
        run
      ),
      call. = FALSE
    )
  }

  html <- normalizePath(html, winslash = "/")
  if (browse) {
    utils::browseURL(html)
  }
  invisible(list(html = html, status = status))
}
