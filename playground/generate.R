# Generates the playground cases: one clean dataset plus deliberately broken
# variants, all conforming to (or violating) playground/data-dict.yaml.
# Run from the repo root: Rscript playground/generate.R

library(nanoparquet)

out_root <- file.path("playground", "cases")

make_customers <- function(n = 200) {
  segments <- c("retail", "wholesale", "internal")
  data.frame(
    customer_id = seq_len(n),
    name = sprintf("Customer %03d", seq_len(n)),
    email = sprintf("customer%03d@example.com", seq_len(n)),
    segment = sample(segments, n, replace = TRUE, prob = c(0.5, 0.4, 0.1)),
    signup_date = as.Date("2023-01-01") + sample(0:700, n, replace = TRUE),
    postcode = sprintf("%05d", sample(10000:99999, n, replace = TRUE)),
    stringsAsFactors = FALSE
  )
}

make_orders <- function(customer_ids, n = 2000) {
  order_date <- as.Date("2024-01-01") + sample(0:540, n, replace = TRUE)
  status <- sample(
    c("pending", "shipped", "delivered", "cancelled"),
    n, replace = TRUE, prob = c(0.15, 0.25, 0.5, 0.1)
  )
  shipped <- status %in% c("shipped", "delivered")
  data.frame(
    order_id = seq_len(n),
    customer_id = sample(customer_ids, n, replace = TRUE),
    order_date = order_date,
    ship_date = ifelse(shipped, order_date + sample(1:7, n, replace = TRUE), NA),
    status = status,
    quantity = sample(1:100, n, replace = TRUE),
    unit_price = round(runif(n, 0.5, 20), 2),
    stringsAsFactors = FALSE
  ) |>
    # keep line_total under the 10,000 credit limit
    transform(unit_price = pmin(unit_price, 10000 / quantity)) |>
    transform(ship_date = as.Date(ship_date, origin = "1970-01-01"))
}

write_case <- function(case, customers, orders) {
  dir <- file.path(out_root, case)
  dir.create(dir, recursive = TRUE, showWarnings = FALSE)
  write_parquet(customers, file.path(dir, "customers.parquet"))
  write_parquet(orders, file.path(dir, "orders.parquet"))
  file.copy(file.path("playground", "data-dict.yaml"), file.path(dir, "data-dict.yaml"),
    overwrite = TRUE
  )
}

set.seed(8419)
customers <- make_customers()
orders <- make_orders(customers$customer_id)

write_case("clean", customers, orders)

# Metadata problems: the dict describes what the data doesn't have.
write_case(
  "meta-problem",
  within(customers, rm(email)),
  transform(orders, unit_price = formatC(unit_price, format = "f", digits = 2))
)

# Rare violations: a handful of bad rows of each kind.
rare_o <- orders
rare_o$order_id[3:5] <- rare_o$order_id[1:3]              # duplicate primary keys
rare_o$quantity[10:14] <- -1                              # below minimum
rare_o$status[20:21] <- "lost"                            # unknown enum value
rare_o$customer_id[30:33] <- 9999                         # foreign key violations
rare_o$ship_date[40] <- rare_o$order_date[40] - 5         # ships before ordered
rare_o$ship_date[41] <- NA                                # delivered, never shipped
rare_o$unit_price[42] <- 10000 / rare_o$quantity[42] + 1  # over credit limit
rare_c <- customers
rare_c$postcode[7] <- "12345-67890"                       # too long
write_case("rare-violations", rare_c, rare_o)

# Common violations: the same problems, but widespread.
bad <- sample.int(nrow(orders), nrow(orders) * 0.15)
common_o <- orders
common_o$customer_id[bad] <- 9999                         # orphaned foreign keys
common_o$quantity[sample.int(nrow(orders), nrow(orders) * 0.2)] <- 0
delivered <- which(common_o$status == "delivered")
common_o$ship_date[sample(delivered, length(delivered) * 0.3)] <- NA
common_o$ship_date[bad] <- common_o$order_date[bad] - 10
common_c <- customers
common_c$name[sample.int(nrow(customers), nrow(customers) * 0.3)] <- NA
common_c$postcode[sample.int(nrow(customers), nrow(customers) * 0.25)] <- "12345-67890"
write_case("common-violations", common_c, common_o)
