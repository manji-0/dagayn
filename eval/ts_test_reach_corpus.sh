#!/usr/bin/env bash
# Prepare the TypeScript corpus of the test-reach gate
# (eval/test_reach_ts_thresholds.yaml): cfwdon's web-ui at the commit M1
# measured, its vitest tests run with v8 coverage into
# DIR/web-ui/coverage/coverage-final.json, and a graph built from the copy.
#
#   eval/ts_test_reach_corpus.sh DIR
#   uv run python eval/run_test_reach_eval.py \
#       --coverage DIR/web-ui/coverage/coverage-final.json --repo DIR/web-ui \
#       --gate --thresholds eval/test_reach_ts_thresholds.yaml
set -euo pipefail

dir=${1:?usage: ts_test_reach_corpus.sh DIR}
url=https://github.com/manji-0/cfwdon.git
commit=943296696ff5b83a00dcf7f4138f344a3650540b

src="$dir/cfwdon"
rm -rf "$src" "$dir/web-ui"
git init -q "$src"
git -C "$src" fetch -q --depth 1 "$url" "$commit"
git -C "$src" checkout -q FETCH_HEAD
cp -R "$src/web-ui" "$dir/web-ui"

cd "$dir/web-ui"
pnpm install --frozen-lockfile
pnpm add -D "@vitest/coverage-v8@$(node -p 'require("vitest/package.json").version')"
printf 'node_modules/\ncoverage/\n' > .gitignore
git init -q
git add -A
git -c user.name=corpus -c user.email=corpus@localhost commit -qm "cfwdon web-ui $commit"
# Without coverage.include, files no test loads would be missing, not at 0.
./node_modules/.bin/vitest run --coverage.enabled --coverage.provider=v8 \
    --coverage.reporter=json --coverage.reportsDirectory=coverage '--coverage.include=src/**'
cd - >/dev/null

uv run dagayn build --repo "$dir/web-ui"
