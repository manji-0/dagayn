# Fixture for cross-language bridge detection tests (R).
# Only canonical bare-name forms are detected; indirect dispatch is out of scope.

system("./target/release/dagayn-core build .")

system2("./scripts/build.sh", args = c("--strict"))

.Call("dagayn_compute")

.External("dagayn_helper")

dyn.load("./target/release/libdagayn.so")

library.dynam("dagayn", "./target/release")

# Dynamic argument — should produce LOW confidence
run_dynamic <- function(cmd) {
  system(cmd)
}
