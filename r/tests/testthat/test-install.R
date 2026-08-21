test_that("every released platform maps to its target triple", {
  expect_equal(target_triple("Darwin", "aarch64"), "aarch64-apple-darwin")
  expect_equal(target_triple("Darwin", "x86_64"), "x86_64-apple-darwin")
  expect_equal(target_triple("Linux", "aarch64"), "aarch64-unknown-linux-musl")
  expect_equal(target_triple("Linux", "x86_64"), "x86_64-unknown-linux-musl")
  expect_equal(target_triple("Windows", "x86_64"), "x86_64-pc-windows-msvc")
})

test_that("a platform without a released binary is an error", {
  expect_error(target_triple("SunOS", "sparc"), "SunOS sparc")
  expect_error(target_triple("Windows", "aarch64"), "no released binary")
})

test_that("this platform is one we build for", {
  expect_true(dd_target() %in% c(
    "aarch64-apple-darwin", "x86_64-apple-darwin",
    "aarch64-unknown-linux-musl", "x86_64-unknown-linux-musl",
    "x86_64-pc-windows-msvc"
  ))
})

test_that("a corrupted download is rejected", {
  dir <- withr::local_tempdir()
  archive <- file.path(dir, "data-dict-cli-x86_64-apple-darwin.tar.xz")
  writeLines("not an archive", archive)
  sums <- paste0(archive, ".sha256")

  writeLines(paste0(strrep("0", 64), "  ", basename(archive)), sums)
  expect_error(verify_sha256(sums, archive), "Checksum mismatch")

  hash <- cli::hash_file_sha256(archive)
  writeLines(paste0(hash, "  ", basename(archive)), sums)
  expect_equal(verify_sha256(sums, archive), hash)
})

test_that("dd_install downloads a working binary", {
  skip_on_cran()
  cache <- withr::local_tempdir()
  withr::local_envvar(R_USER_CACHE_DIR = cache)

  path <- dd_install(quiet = TRUE)
  expect_true(file.exists(path))
  expect_match(system2(path, "--version", stdout = TRUE), "^data-dict ")
})
