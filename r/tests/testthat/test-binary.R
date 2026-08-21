test_that("DATA_DICT wins over an installed binary", {
  bin <- withr::local_tempfile()
  file.create(bin)
  withr::local_envvar(DATA_DICT = bin)
  expect_equal(dd_path(), bin)
})

test_that("DATA_DICT pointing at nothing is an error", {
  withr::local_envvar(DATA_DICT = "/no/such/data-dict")
  expect_error(dd_path(), "does not exist")
})

test_that("a missing binary points at dd_install()", {
  cache <- withr::local_tempdir()
  withr::local_envvar(DATA_DICT = NA, R_USER_CACHE_DIR = cache, PATH = cache)
  expect_error(dd_path(), "dd_install")
  expect_equal(dd_path(check = FALSE), "")
})

test_that("the install directory sits under the cache", {
  cache <- withr::local_tempdir()
  withr::local_envvar(R_USER_CACHE_DIR = cache)
  expect_equal(dd_dir(), file.path(cache, "R", "datadict"))
})

test_that("dd_run captures the binary's output", {
  skip_if_no_cli()
  run <- dd_run("--version")
  expect_equal(run$status, 0L)
  expect_match(run$output, "data-dict", all = FALSE)
})

test_that("dd_run can echo while it captures", {
  skip_if_no_cli()
  expect_output(run <- dd_run("--version", echo = TRUE), "data-dict")
  expect_match(run$output, "data-dict", all = FALSE)
})

test_that("dd_run reports the output on a non-zero exit", {
  skip_if_no_cli()
  expect_error(dd_run(c("validate-spec", "/no/such/dict.yaml")), "status 1")

  run <- run_binary(c("validate-spec", "/no/such/dict.yaml"))
  expect_true(run$status != 0)
  expect_true(length(run$output) > 0)
})
