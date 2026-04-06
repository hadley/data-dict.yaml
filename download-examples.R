repos <- list(
  elevators = "hadley/elevators",
  foodbank = "hadley/foodbank",
  `loan-application` = "hadley/loan-application",
  otters = "hadley/otters",
  dabstep = "hadley/dabstep"
)

for (name in names(repos)) {
  repo <- repos[[name]]
  url <- paste0(
    "https://raw.githubusercontent.com/", repo,
    "/refs/heads/main/data-dict.yaml"
  )
  dest <- file.path("examples", paste0(name, ".yaml"))
  download.file(url, dest)
  contents <- readLines(dest)
  source_url <- paste0("https://github.com/", repo)
  writeLines(c(paste0("# source: ", source_url), "", contents), dest)
}
