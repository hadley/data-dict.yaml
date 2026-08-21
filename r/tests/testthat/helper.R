skip_if_no_cli <- function() {
  skip_if(dd_path(check = FALSE) == "", "no data-dict binary available")
}

# A dictionary for one table of otters, and the parquet file it describes.
write_fixture <- function(dir, species = c("lutra", "pteronura")) {
  dir.create(dir, recursive = TRUE, showWarnings = FALSE)
  writeLines(c(
    '$version: "0.1.0"',
    "$learn_more: https://data-dict.tidyverse.org/",
    "description: Each row is one otter in the census.",
    "tables:",
    "  - name: otters",
    "    source:",
    "      parquet: otters.parquet",
    "    columns:",
    "      - name: id",
    "        type: number(id)",
    "        constraints: [primary_key]",
    "        description: Unique identifier for the otter.",
    "        examples: [1, 2, 3, 4, 5]",
    "      - name: species",
    "        type: enum",
    "        values: [lutra, pteronura]",
    "        constraints: [required]",
    "        description: The otter's species."
  ), file.path(dir, "data-dict.yaml"))
  nanoparquet::write_parquet(
    data.frame(id = seq_along(species), species = species),
    file.path(dir, "otters.parquet")
  )
  dir
}
