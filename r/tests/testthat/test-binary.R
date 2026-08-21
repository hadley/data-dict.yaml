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
