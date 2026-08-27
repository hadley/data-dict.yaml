base_url <- "https://github.com/tidyverse/data-dict/releases"

dd_target <- function() {
  target_triple(Sys.info()[["sysname"]], R.version$arch)
}

target_triple <- function(sysname, arch) {
  triple <- switch(
    paste(sysname, arch),
    "Darwin aarch64" = "aarch64-apple-darwin",
    "Darwin x86_64" = "x86_64-apple-darwin",
    "Linux aarch64" = "aarch64-unknown-linux-musl",
    "Linux arm64" = "aarch64-unknown-linux-musl",
    "Linux x86_64" = "x86_64-unknown-linux-musl",
    "Windows x86_64" = "x86_64-pc-windows-msvc",
    NULL
  )
  if (is.null(triple)) {
    stop("data-dict has no released binary for ", sysname, " ", arch, ". ",
         "See https://data-dict.tidyverse.org/install.html to build from ",
         "source.", call. = FALSE)
  }
  triple
}

#' Download the `data-dict` binary
#'
#' Downloads the release archive for this platform, checks it against the
#' published SHA-256, and unpacks the binary into
#' `tools::R_user_dir("datadict", "cache")`.
#'
#' @param version Release to install, e.g. `"0.0.1"` (a leading `v` is also
#'   accepted). Defaults to the latest release.
#' @param force Whether to download again when a binary is already installed.
#' @param quiet Whether to suppress the download progress bar and messages.
#' @return The path to the installed binary, invisibly.
#' @export
#' @examplesIf FALSE
#' dd_install()
dd_install <- function(version = "latest", force = FALSE, quiet = FALSE) {
  dest <- file.path(dd_dir(), bin_name())
  if (file.exists(dest) && !force) {
    if (!quiet) {
      message("data-dict is already installed at ", dest,
              "\nUse force = TRUE to download it again.")
    }
    return(invisible(dest))
  }

  target <- dd_target()
  asset <- if (target == "x86_64-pc-windows-msvc") {
    paste0("data-dict-cli-", target, ".zip")
  } else {
    paste0("data-dict-cli-", target, ".tar.xz")
  }
  url <- if (identical(version, "latest")) {
    file.path(base_url, "latest/download", asset)
  } else {
    file.path(base_url, "download", paste0("v", sub("^v", "", version)), asset)
  }

  archive <- file.path(tempfile("datadict-"), asset)
  dir.create(dirname(archive), recursive = TRUE)
  on.exit(unlink(dirname(archive), recursive = TRUE), add = TRUE)
  download(url, archive, quiet)
  sums <- paste0(archive, ".sha256")
  download(paste0(url, ".sha256"), sums, quiet)
  verify_sha256(sums, archive)

  exdir <- tempfile("datadict-")
  dir.create(exdir, recursive = TRUE)
  on.exit(unlink(exdir, recursive = TRUE), add = TRUE)
  if (endsWith(asset, ".zip")) {
    utils::unzip(archive, exdir = exdir)
  } else {
    utils::untar(archive, exdir = exdir)
  }

  # The tar archives wrap everything in a directory named after the target,
  # the zip does not, so find the binary rather than assume where it landed.
  found <- list.files(exdir, pattern = "^data-dict(\\.exe)?$",
                      recursive = TRUE, full.names = TRUE)
  if (length(found) != 1) {
    stop("Found ", length(found), " data-dict binaries in ", asset,
         ", expected exactly one.", call. = FALSE)
  }

  dir.create(dd_dir(), recursive = TRUE, showWarnings = FALSE)
  if (!file.copy(found, dest, overwrite = TRUE)) {
    stop("Failed to install the data-dict binary into ", dd_dir(),
         call. = FALSE)
  }
  Sys.chmod(dest, "0755")

  if (!quiet) {
    installed <- system2(dest, "--version", stdout = TRUE)
    message("Installed ", installed, " to ", dest)
  }
  invisible(dest)
}

download <- function(url, path, quiet) {
  status <- tryCatch(
    utils::download.file(url, path, mode = "wb", quiet = quiet),
    error = function(err) conditionMessage(err)
  )
  if (!identical(status, 0L)) {
    stop("Failed to download ", url,
         if (is.character(status)) paste0("\n", status), call. = FALSE)
  }
  invisible(path)
}

verify_sha256 <- function(sums, archive) {
  # The file holds a `sha256sum` line: the hash, then the file name.
  line <- trimws(readLines(sums, warn = FALSE)[[1]])
  expected <- strsplit(line, "\\s+")[[1]][[1]]
  actual <- cli::hash_file_sha256(archive)
  if (!identical(tolower(expected), tolower(actual))) {
    stop("Checksum mismatch for ", basename(archive),
         "\n  expected: ", expected, "\n  found:    ", actual, call. = FALSE)
  }
  invisible(actual)
}
