#!/bin/sh
# Builds the key inputs for one target checkout, without dagayn.
#
#   build_inputs.sh BIN TARGET OUT NAME
#
# BIN holds scip-go (v0.2.7), scipdump, sciprels, and gofuncs. TARGET is the
# harness's target (`make_target`). OUT gets NAME.scip, NAME.occ.tsv,
# NAME.rels.tsv, NAME.funcs.tsv, and NAME.cover1.json and NAME.cover2.json:
# two per-test coverage runs of every package, without -short.
set -eu
BIN=$1 TARGET=$2 OUT=$3 NAME=$4
KEYS=$(cd "$(dirname "$0")" && pwd)
G7KEYS=$KEYS/../../g7_go/keys
mkdir -p "$OUT"
cd "$TARGET"
GOTOOLCHAIN=go1.25.3 "$BIN/scip-go" --output "$OUT/$NAME.scip"
"$BIN/scipdump" "$OUT/$NAME.scip" > "$OUT/$NAME.occ.tsv"
"$BIN/sciprels" "$OUT/$NAME.scip" > "$OUT/$NAME.rels.tsv"
"$BIN/gofuncs" "$TARGET" > "$OUT/$NAME.funcs.tsv"
for i in 1 2; do
  GOTOOLCHAIN=go1.25.3 python3 "$G7KEYS/per_test_cover.py" "$TARGET" "$OUT/$NAME.funcs.tsv" \
    "$OUT/$NAME.cover$i.json"
done
