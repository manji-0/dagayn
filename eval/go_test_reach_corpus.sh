#!/usr/bin/env bash
# Prepare the Go corpus of the test-reach gate (eval/test_reach_go_thresholds.yaml):
# a copy of testify v1.12.1 from the module proxy, its tests run with coverage
# into DIR/testify/cover.func, and a graph built from the copy.
#
#   eval/go_test_reach_corpus.sh DIR
#   uv run python eval/run_test_reach_eval.py --coverage DIR/testify/cover.func \
#       --repo DIR/testify --gate --thresholds eval/test_reach_go_thresholds.yaml
set -euo pipefail

dir=${1:?usage: go_test_reach_corpus.sh DIR}
module=github.com/stretchr/testify@v1.12.1
# suite's own tests panic under Go 1.24; http has no tests.
packages=(./assert/... ./require/... ./internal/... ./mock/...)
# They stat paths relative to the source tree, which the copy does not keep.
skip='TestDirExists|TestNoDirExists'

src=$(cd "$dir" && GOFLAGS=-mod=mod go mod download -json "$module" | python3 -c 'import json, sys; print(json.load(sys.stdin)["Dir"])')
repo="$dir/testify"
rm -rf "$repo"
cp -R "$src" "$repo"
chmod -R u+w "$repo"

cd "$repo"
git init -q
git add -A
git -c user.name=corpus -c user.email=corpus@localhost commit -qm "$module"

coverpkg=$(IFS=,; echo "${packages[*]}")
go test -count=1 -skip "$skip" -coverpkg="$coverpkg" -coverprofile=cover.out "${packages[@]}"
go tool cover -func=cover.out -o cover.func
cd - >/dev/null

uv run dagayn build --repo "$repo"
