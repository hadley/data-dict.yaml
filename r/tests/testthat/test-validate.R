test_that("a clean dataset validates and writes a report", {
  skip_if_no_cli()
  skip_if_not_installed("nanoparquet")
  dict <- write_fixture(withr::local_tempdir())
  html <- withr::local_tempfile(fileext = ".html")

  report <- dd_validate_data(dict, html = html, browse = FALSE)

  expect_equal(report$status, 0)
  expect_equal(report$html, normalizePath(html, winslash = "/"))
  expect_true(file.size(html) > 0)
  expect_match(readLines(html, n = 1, warn = FALSE), "<!", fixed = TRUE)
})

test_that("a value outside the dictionary's enum fails, with a report", {
  skip_if_no_cli()
  skip_if_not_installed("nanoparquet")
  dict <- write_fixture(withr::local_tempdir(), species = "enhydra")
  html <- withr::local_tempfile(fileext = ".html")

  report <- dd_validate_data(dict, table = "otters", html = html,
                             browse = FALSE)

  expect_true(report$status != 0)
  expect_true(file.exists(html))
})

test_that("a dictionary that cannot be read is an R error", {
  skip_if_no_cli()
  dir <- withr::local_tempdir()
  writeLines("tables: [", file.path(dir, "data-dict.yaml"))
  expect_error(
    dd_validate_data(dir, html = withr::local_tempfile(fileext = ".html"),
                     browse = FALSE),
    "wrote no report"
  )
})
