# Fixture for file-I/O bridge detection tests (R).

read_config <- function() {
  readLines("config.yaml")
}

write_output <- function() {
  writeLines("done", "output.txt")
}

load_data <- function() {
  read.csv("data/training.csv")
}

save_results <- function(df) {
  write.csv(df, "results/output.csv")
}

read_dynamic <- function(path) {
  # Dynamic path — LOW confidence edge
  readLines(path)
}
