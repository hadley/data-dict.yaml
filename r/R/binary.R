dd_dir <- function() {
  tools::R_user_dir("datadict", "cache")
}

#' Locate the `data-dict` binary
#'
#' Searched in order: the `DATA_DICT` environment variable, the copy installed
#' by [dd_install()] in the package's cache directory, then the `PATH`.
#'
#' @param check Whether to throw an error when no binary is found. With
#'   `FALSE`, return `""` instead.
#' @return Path to the binary, or `""` if it was not found and `check` is
#'   `FALSE`.
#' @export
#' @examples
#' dd_path(check = FALSE)
dd_path <- function(check = TRUE) {
  from_env <- Sys.getenv("DATA_DICT", "")
  if (nzchar(from_env)) {
    if (!file.exists(from_env)) {
      stop("DATA_DICT points at a file that does not exist: ", from_env,
           call. = FALSE)
    }
    return(from_env)
  }

  installed <- file.path(dd_dir(), bin_name())
  if (file.exists(installed)) {
    return(installed)
  }

  on_path <- Sys.which("data-dict")[[1]]
  if (nzchar(on_path)) {
    return(on_path)
  }

  if (check) {
    stop("No data-dict binary found. ",
         "Run datadict::dd_install() to download one.", call. = FALSE)
  }
  ""
}

bin_name <- function() {
  if (.Platform$OS.type == "windows") "data-dict.exe" else "data-dict"
}

#' Run the `data-dict` binary
#'
#' Both output streams are captured, interleaved as the binary wrote them.
#' With `echo = TRUE` they are also printed as they arrive, so a long run
#' shows its progress.
#'
#' A non-zero exit status is an error, carrying the captured output as the
#' message.
#'
#' @param args Character vector of command line arguments.
#' @param echo Whether to print the binary's output while it runs, on top of
#'   capturing it.
#' @param ... Passed on to [processx::run()], e.g. `timeout` or `wd`.
#' @return A list with the exit `status` and the captured `output`, a character
#'   vector of lines, invisibly.
#' @export
#' @examplesIf FALSE
#' dd_run("--version")
dd_run <- function(args, echo = FALSE, ...) {
  result <- run_binary(args, echo = echo, ...)
  if (result$status != 0) {
    stop(
      run_failure(paste0("data-dict failed with status ", result$status, ":"),
                  result),
      call. = FALSE
    )
  }
  invisible(result)
}

run_binary <- function(args, echo = FALSE, ...) {
  result <- processx::run(
    dd_path(),
    as.character(args),
    error_on_status = FALSE,
    stderr_to_stdout = TRUE,
    echo = echo,
    ...
  )
  list(status = result$status, output = split_lines(result$stdout))
}

run_failure <- function(headline, result) {
  paste(c(headline, result$output), collapse = "\n")
}

split_lines <- function(text) {
  if (!nzchar(text)) {
    return(character())
  }
  strsplit(sub("\r?\n$", "", text), "\r?\n")[[1]]
}
