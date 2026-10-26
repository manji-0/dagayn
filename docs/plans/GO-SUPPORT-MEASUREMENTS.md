# Go support measurements

<!-- derived-from ./TEST-REACH-TARGET.md#evaluation -->

## Question

What does dagayn answer about Go code, measured against coverage, before
and after Go tests and Go packages are in the graph? These are the
measurements of work items M2, G1, and G2 of the first-class languages plan
(`docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md`, PR #217). Each item ships
only if it meets the bar the plan sets for it.

## Corpora

| Repository | Version | Source | Go | Coverage run |
|---|---|---|---|---|
| testify | v1.12.1 | copy of the Go module cache, no history | 1.24.13 | `assert`, `require`, `internal` |
| compress | v1.20.1 (`5d880f2`) | clone of `github.com/klauspost/compress` | 1.25.3 | all packages, `-short` |
| scip-go | v0.2.7 | clone of `github.com/sourcegraph/scip-go` | 1.25.3 | all packages |

Every run is `go test -coverpkg=<the measured packages> -coverprofile=...`
then `go tool cover -func`, so a function counts as run when any test
package ran any of its statements. testify's run leaves out `http`, `mock`,
and `suite` (their dependencies are not in the module cache, and `suite`
panicked under Go 1.24) and skips `TestDirExists` and `TestNoDirExists`,
which fail outside the source tree, as the plan's baseline did. scip-go's
`internal/git` `TestInferRepo` fails in a clone without the upstream remote;
the other packages' coverage stands. Go 1.25.3 came from
`GOTOOLCHAIN=go1.25.3` (the module cache's `golang.org/toolchain`). scip-go
v0.2.7 is the last tag that builds with Go 1.25; v0.2.8 needs 1.26.

Graphs were built with `dagayn build --repo COPY --force` on copies in a
scratch directory. Before each measurement the graph's metadata was
checked: `last_build_type` `full` and the `go` extractor version of the
build under test.

## M2: test reach against Go coverage

<!-- derived-from ./TEST-REACH-TARGET.md#evaluation -->

`eval/run_test_reach_eval.py` reads a `go tool cover -func` listing
(`--coverage cover.func --repo GO_REPO`). A listing line names the
package's import path, the file, and the line of the `func` keyword; the
harness maps the import path to a directory through the repository's
`go.mod` files and matches a Go `Function` node on file and first line. A
function in a file the build left out (another `GOARCH`'s `_amd64.go`, a
build tag) is in no listing and so in no population.

Bar: the harness reproduces the plan's testify baseline and runs on a second
repository with git history.

| Repository (go extractor 8) | Functions | Called untested | Ran, called untested | `no_callers` / `no_test_path` / `beyond_limit` (ran) | Untested precision | Recall | Tested precision |
|---|---|---|---|---|---|---|---|
| testify | 735 | 720 | **277** of 292 | 111 / 166 / 0 | 0.615 | 1.000 | 1.000 (15) |
| compress | 1,150 | 946 | 786 of 978 | 425 / 355 / 6 | 0.169 | 0.930 | 0.941 (204) |
| scip-go | 137 | 137 | 110 of 110 | 41 / 69 / 0 | 0.197 | 1.000 | none |

testify matches the plan exactly (735, 720, 277 = 111 + 166). compress is
the second repository: 1,150 of the 1,155 functions its listing names match
a node. Met. No floor is set: Go has no CI coverage run, and these numbers
only show that almost everything is called untested.

## G1: Go tests in the graph

<!-- derived-from #m2-test-reach-against-go-coverage -->

Go extractor 9: every node of a `_test.go` file is test code, and its
top-level `TestX`, `BenchmarkX`, `FuzzX`, and `ExampleX` functions are
`Test` nodes whose calls give `TESTED_BY` edges.

| Repository | `Test` nodes | `is_test` nodes | `TESTED_BY` | Ran, called untested (8 → 9) |
|---|---|---|---|---|
| testify | 0 → 479 | 0 → 813 | 0 → 1,343 | 277 → 277 |
| compress | 0 → 805 | 0 → 1,148 | 0 → 932 | 786 → 784 |
| scip-go | 0 → 35 | 0 → 65 | 0 → 2 | 110 → 110 |

Alone, G1 moves test reach by two functions, as the plan expected: a Go
test calls code of another file of its package by its bare name, and
nothing binds such a call yet, so the `TESTED_BY` edges that survive name
helpers of the test's own file. The review case `untested_change_go`
passes; `tests_to_run_go` and the negative `neg_untested_reached_go` need
the calls bound, and are measured with G2. The bar that the 277 falls is
read with G2, as the plan says to measure the two together: 277 → 42.

## G2: the package as a namespace

<!-- derived-from #g1-go-tests-in-the-graph -->

Go extractor 10. Each File node declares its package's import path
(`go.mod`'s module path and the directory below it) as its namespace,
`path_test` for an external test package, and none for a
`//go:build ignore` file. Only a plain call binds by name through it, and
only to a top-level function or type. Before widening visibility the
extractor had to tell plain calls apart: a call through a package
qualifier (`assert.Equal(t, ...)`) carried no mark, so same-file resolution
bound it to the caller's own `Equal`. testify's `require` had 304 such
self-calls, and compress 5 (`snappy.Decode` calling `s2.Decode`). Such calls
now record `receiver` (and `import_path` for an import) and are never bound
by name; a call on a value of unknown type is not bound by name either, as
in Rust.

Resolving these calls exposed three Go rules of the return-type pass that
were too wide, and each is scoped to Go's own rule rather than extended:
a typed method is looked up in its type's directory (huff0's `Scratch` made
fse's `s.prepare(in)` ambiguous), a qualified return type
(`io.ReadCloser`) is the imported package's (11 tests of `flate`, `lzw`, and
`zlib` had bound `reader.Close()` to `zip/reader.go::ReadCloser.Close`), and
an unqualified one is a type of the function's own directory. No new
receiver-type inference was added.

| Clause of the bar | Before (go 9) | After (go 10) | Met |
|---|---|---|---|
| compress: unresolved plain calls to a function of the caller's directory, under 300 | 3,058 | 166 | yes |
| compress: production functions with no caller, under 0.15 | 0.463 (623 of 1,345) | 0.312 (419) | **no** |
| hand check of 30 newly bound edges: none into another package | | 0 of 30 | yes |
| testify: functions that ran called untested, at most 92 | 277 | 42 | yes |
| testify: tested precision at least 0.92 | 1.000 (15) | 0.992 (252) | yes |
| review `unchanged_caller_go` | fails | passes | yes |

The plan's compress numbers were 3,098 and 0.478 for the same version;
these counts exclude test, `vendor`, example, and generated paths
(`_gen.go`, `.pb.go`) the same way but differ slightly in what they skip.

### Every newly bound edge

On compress, 3,665 `CALLS` edges are bound that go 9 left bare (2,834
plain, 831 on a typed receiver); on testify, 1,767 (1,442 and 325). None
binds outside the caller's directory. Every plain one has the same package
clause on both sides and names the only top-level function or type of the
name in that package: `lzw`'s external test package declares a reference
`NewReader` of its own (`ref_aldus_test.go`), and its tests bind to that
one while the package's tests bind to `lzw/reader.go::NewReader`. The edges go 10 no longer binds, compared with go 9,
are wrong edges removed: on compress 5 self-calls, 11 package calls bound
to a same-named function of the file (`flate.NewReader` in `gzip/gunzip.go`
to `gzip`'s own `NewReader`), 74 rebound to the right method (`w.Write` on
a `flate.Writer` had bound to a test's `syncBuffer.Write`), and 172 calls
on a value of unknown type that had bound by name to a method of the file.
Most of those were wrong (test fakes such as `syncBuffer.Close` and
`mockRWCloseNotify.Header`, and `s2/cmd/internal/readahead/reader.go:463`
`c.Close()` bound to `zlib/reader.go::reader.Close`); some were right and
are lost, such as `huff0/compress.go:631` `huffNode[n].nbBits()` and
`s2/cmd/internal/readahead/reader.go:326` `b.read(a.in)`. The rule that
loses them is Rust's: a call on a value of unknown type is not bound by
name. On testify the removed edges are 154 self-calls and 35 such name
bindings (`expectedValue.Len()`, a `reflect.Value`'s, bound to
`assert.Len`).

### Hand check

A seeded random sample of 30 newly bound compress edges, 15 plain and 15
typed. A plain row was checked against the package clause of both files
and the directory's other declarations; a typed row against the
declaration of its receiver (in parentheses).

| # | Call (file:line) | Caller | Bound to | Right |
|---|---|---|---|---|
| 1 | `flate/deflate.go:433` `newHuffmanEncoder(...)` | `compressor.deflateLazy` | `flate/huffman_code.go::newHuffmanEncoder` | yes |
| 2 | `flate/writer_test.go:302` `w.Flush()` (`w, err := NewWriter(ew, l)`, `*Writer`) | `TestWriteError` | `flate/deflate.go::Writer.Flush` | yes |
| 3 | `gzhttp/compress_test.go:58` `assertEqual(...)` | `TestParseEncodings` | `gzhttp/asserts_test.go::assertEqual` | yes |
| 4 | `gzhttp/compress_test.go:520` `assertNotEqual(...)` | `TestGzipHandlerContentLength` | `gzhttp/asserts_test.go::assertNotEqual` | yes |
| 5 | `gzhttp/compress_test.go:1004` `assertEqual(...)` | `TestContentTypes` | `gzhttp/asserts_test.go::assertEqual` | yes |
| 6 | `huff0/compress.go:167` `s.prepare(in)` (`s *Scratch`) | `EstimateSizes` | `huff0/huff0.go::Scratch.prepare` | yes |
| 7 | `huff0/compress_test.go:163` `ReadTable(...)` | `TestCompressRegression` | `huff0/decompress.go::ReadTable` | yes |
| 8 | `s2/decode_test.go:42` `Decode(nil, in)` | `TestDecodeRegression` | `s2/decode.go::Decode` | yes |
| 9 | `s2/reader_test.go:15` `NewWriter(&buf)` | `TestLeadingSkippableBlock` | `s2/writer.go::NewWriter` | yes |
| 10 | `s2/s2_test.go:1242` `NewReader(&buf)` | `testWriterRoundtrip` | `s2/reader.go::NewReader` | yes |
| 11 | `s2/s2_test.go:1839` `EncodeBest(nil, data)` | `benchFile` | `s2/encode.go::EncodeBest` | yes |
| 12 | `s2/writer_test.go:173` `dec.Reset(&buf)` (`dec := NewReader(nil)`, `*Reader`) | `TestEncoderRegression` | `s2/reader.go::Reader.Reset` | yes |
| 13 | `s2/writer_test.go:439` `d.Reset(&dst)` (`d := NewReader(nil)`) | `TestWriterPadding` | `s2/reader.go::Reader.Reset` | yes |
| 14 | `zip/reader_test.go:227` `timeZone(0)` | the file's test table | `zip/struct.go::timeZone` | yes |
| 15 | `zip/reader_test.go:457` `timeZone(-7*time.Hour)` | the file's test table | `zip/struct.go::timeZone` | yes |
| 16 | `zip/reader_test.go:2015` `zw.CreateHeader(...)` (`zw := NewWriter(&buf)`, `*Writer`) | `TestNameDecoderError` | `zip/writer.go::Writer.CreateHeader` | yes |
| 17 | `zstd/decoder_test.go:390` `dec.Close()` (`NewReader(...)`, `*Decoder`) | `TestNewDecoderFrameSize` | `zstd/decoder.go::Decoder.Close` | yes |
| 18 | `zstd/decoder_test.go:1478` `dec.Close()` (`NewReader(nil, ...)`) | `BenchmarkDecoder_DecodeAll` | `zstd/decoder.go::Decoder.Close` | yes |
| 19 | `zstd/decoder_test.go:1493` `dec.DecodeAll(in, nil)` (same `dec`) | `BenchmarkDecoder_DecodeAll` | `zstd/decoder.go::Decoder.DecodeAll` | yes |
| 20 | `zstd/decoder_test.go:2238` `IgnoreChecksum(true)` | `TestDecoderResetWithOptions` | `zstd/decoder_options.go::IgnoreChecksum` | yes |
| 21 | `zstd/enc_better.go:426` `load6432(src, s+matched)` | `betterFastEncoder.Encode` | `zstd/zstd.go::load6432` | yes |
| 22 | `zstd/enc_jobs_test.go:53` `WithLowerEncoderMem(true)` | `getConcBlockOpts` | `zstd/encoder_options.go::WithLowerEncoderMem` | yes |
| 23 | `zstd/enc_jobs_test.go:140` `dec.Close()` (`NewReader(nil)`) | `TestConcurrentBlocks_Regression` | `zstd/decoder.go::Decoder.Close` | yes |
| 24 | `zstd/enc_jobs_test.go:170` `enc.EncodeAll(in, nil)` (`NewWriter(nil, ...)`, `*Encoder`) | `TestConcurrentBlocks_Regression` | `zstd/encoder.go::Encoder.EncodeAll` | yes |
| 25 | `zstd/enc_jobs_test.go:1298` `dec.Close()` (`NewReader(...)`) | `TestConcurrentBlocks_StreamDecode` | `zstd/decoder.go::Decoder.Close` | yes |
| 26 | `zstd/encoder_options_test.go:163` `enc.Close()` (`NewWriter(&buf, ...)`) | `TestEncoderResetWithOptions` | `zstd/encoder.go::Encoder.Close` | yes |
| 27 | `zstd/encoder_test.go:262` `enc.EncodeAll(in, nil)` (`NewWriter(...)`) | `TestEncoderRegression` | `zstd/encoder.go::Encoder.EncodeAll` | yes |
| 28 | `zstd/encoder_test.go:614` `enc.Close()` (`NewWriter(pw, ...)`) | `testEncoderRoundtrip` | `zstd/encoder.go::Encoder.Close` | yes |
| 29 | `zstd/encoder_test.go:825` `dec.Close()` (`NewReader(nil)`) | `TestEncoderReadFrom` | `zstd/decoder.go::Decoder.Close` | yes |
| 30 | `zstd/internal/xxhash/xxhash_other.go:67` `round(v1, ...)` | `writeBlocks` | `zstd/internal/xxhash/xxhash.go::round` | yes |

### Test reach after G2

| Repository (go extractor 10) | Functions | Called untested | Ran, called untested | `no_callers` / `no_test_path` / `beyond_limit` (ran) | Untested precision | Recall | Tested precision |
|---|---|---|---|---|---|---|---|
| testify | 735 | 483 | **42** of 292 | 39 / 3 / 0 | 0.913 | 0.995 | 0.992 (252) |
| compress | 1,150 | 517 | 360 of 978 | 235 / 124 / 1 | 0.304 | 0.913 | 0.976 (633) |
| scip-go | 137 | 127 | 100 of 110 | 27 / 73 / 0 | 0.213 | 1.000 | 1.000 (10) |

testify's 39 remaining `no_callers` are `assert`'s functions that `require`
calls through the `assert.` qualifier (`assertion_format.go::Errorf`):
calls into another package, which the next item binds. No floor is set:
untested precision is 0.913 on testify but 0.304 on compress, and Go has
no coverage run in CI.

### Why the no-caller bar is not met

The 419 production functions of compress with no caller, by what names
them:

| Cause | Functions | Example | What would bind it |
|---|---|---|---|
| Method calls on a value whose type the extractor does not give (an unknown receiver, a method promoted from an embedded struct, a package variable) | 219 | `flate/fast_encoder.go::fastGen.addBlock`, called as `e.addBlock(...)` on a `fastEncL1` that embeds `fastGen` | SCIP (receiver inference is frozen) |
| No call by name at all (function values, tables, interface dispatch, `main`) | 79 | `dict/builder.go::hash4`, assigned to a variable | SCIP, or not a call |
| The same function in two build-tag variants, so a plain call has two candidates | 57 | `huff0/decompress_asm.go:129` calls `decompress4x_main_loop_asm`, declared in `decompress_amd64.go` and `decompress_arm64.go` | build tags are out of scope |
| Calls into another package of the module | 52 | `dict/builder.go::BuildZstdDict`, called as `dict.BuildZstdDict(...)` | G3, imports bound to packages |
| `init`, which only the runtime calls | 7 | `flate/huffman_bit_writer.go::init` | not a call |
| Functions on a typed or unknown receiver (`gzkp.Levels`) | 5 | `zip/register.go::RegisterCompressor` | G3 |

G2 binds plain calls within a package, and 166 of those stay unbound (the
build-tag variants above). The bar's other clauses measure that mechanism
and are met; this one measures every way a function is called, and four
fifths of what is left is calls on values and into other packages, which
the plan assigns to SCIP and to G3. Part of the drop to 0.312 is also the
removal of wrong edges: the self-calls through a package qualifier had
counted as callers. G2 ships with this clause recorded as not met.
