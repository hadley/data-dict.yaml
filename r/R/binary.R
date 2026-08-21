#' Where `datadict` keeps the `data-dict` binary
#'
#' @return Path to the directory the binary is installed into by
#'   [dd_install()]. The directory may not exist yet.
#' @export
#' @examples
#' dd_dir()
dd_dir <- function() {
  tools::R_user_dir("datadict", "cache")
}

#' Locate the `data-dict` binary
#'
#' Searched in order: the `DATA_DICT` environment variable, the copy installed
#' by [dd_install()] in [dd_dir()], then the `PATH`.
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
#' @param args Character vector of command line arguments.
#' @param ... Passed on to [system2()], e.g. `stdout` to capture output.
#' @return The exit status, invisibly, unless `...` redirects the output, in
#'   which case whatever [system2()] returns.
#' @export
#' @examples
#' if (nzchar(dd_path(check = FALSE))) dd_run("--version")
dd_run <- function(args, ...) {
  system2(dd_path(), args, ...)
}
