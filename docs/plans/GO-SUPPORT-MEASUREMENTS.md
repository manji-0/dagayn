# Go support measurements

<!-- derived-from ./TEST-REACH-TARGET.md#evaluation -->

## Question

What does dagayn answer about Go code, measured against coverage, before
and after Go tests and Go packages are in the graph? These are the
measurements of work items M2, G1, G2, G2b, and G3 of the first-class languages plan
(`docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md`, PR #217). Each item ships
only if it meets the bar the plan sets for it.

## Corpora

| Repository | Version | Source | Go | Coverage run |
|---|---|---|---|---|
| testify | v1.12.1 | copy of the Go module cache, no history | 1.24.13 | `assert`, `require`, `internal` |
| compress | v1.20.1 (`5d880f2`) | clone of `github.com/klauspost/compress` | 1.25.3 | all packages, `-short` |
| scip-go | v0.2.7 | clone of `github.com/sourcegraph/scip-go` | 1.25.3 | all packages |
| kong | v1.15.0 | copy of the Go module cache, no history | | none (`alecthomas/assert/v2` is not in the cache) |

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

## G2b: receivers typed by their declarations

<!-- derived-from #why-the-no-caller-bar-is-not-met -->

Go extractor 11. Go writes the types of its values down, and the extractor
now reads more of what is written: the type of a struct field (`s.br`,
`br := &s.br`, a path of fields `e.state.jobs`), of an element of a slice,
array, or map (`b[i]` on `b []hcode`, `for _, tok := range in`), of a
channel's values (`for block := range ch`, `<-ch`), of `make([]T, n)`,
and the methods a struct gets from the structs it embeds (`e.addBlock()` on
a `fastEncL1` that embeds `fastGen`). A struct of another file is finished
in post-processing from its Class node's `field_types` and `embedded`.
Another package's type (`d *zstd.Decoder`) is bound in that package
through its import path (`receiver_package`). None of this infers a type:
every rule reads a declaration, and a selector Go itself would reject as
ambiguous (two members of one name at one embedding depth) binds nothing.

The ground truth is scip-go v0.2.7, built from the clone above, indexed
with `GOTOOLCHAIN=go1.25.3`: compress for darwin/arm64, linux/amd64, and
`-tags noasm` (so every build-tag variant of a file is in some index),
testify for darwin/arm64 and linux/amd64. A script joins each graph edge to
the SCIP reference of the method's name at the call (on the edge's line or
the next lines of a chain split over lines): an edge is right when that
reference's symbol is defined where the target node starts.

### What the 219 were

Of compress's 419 production functions with no caller after G2, G2 put 219
down to method calls on values of unknown type. SCIP says what they are:

| | compress | testify |
|---|---|---|
| Unknown-value functions with no caller (G2) | 219 | 24 |
| No direct reference in any SCIP build (interface dispatch, method values) | 127 | 4 |
| No SCIP symbol (a file no build compiles) | 1 | 0 |
| Referenced directly | 91 | 20 |
| Of those, given a caller by go 11 | **71** | **17** |

127 of the 219 cannot get a caller from any rule that binds a call to the
method it names: their only uses are calls through an interface
(`io.Reader.Read`, the `flate` package's `encoder.Encode`) or method
values. The 91 referenced ones, by the shape of their first call site:

| Shape | Example | compress: referenced / now with a caller | testify |
|---|---|---|---|
| A variable read from a field | `wr := &b.wr; wr.reset()` | 20 / 20 | |
| A field of a struct of another file | `s.bw.close()` | 16 / 15 | 1 / 1 |
| An element of a declared slice or array | `huffNode[n].setCount()` | 11 / 11 | 1 / 1 |
| A method of an embedded struct | `e.addBlock()` on `fastEncL1` | 10 / 10 | 9 / 9 |
| A `range` variable | `for _, tok := range in` | 9 / 9 | |
| A call's result | `snapref.NewBufferedWriter(&buf).Write()` | 14 / 1 | 8 / 6 |
| Another local | `case b := <-a.reuse` | 10 / 5 | 1 / 0 |
| A method value | `e.initialize` passed as a function | 1 / 0 | |

Most of the call results left are calls into another package
(`snapref.NewBufferedWriter`, `s2.MakeDict`, `zip.NewWriter` from the
external test package), which G3 binds, and methods declared twice under
build tags (`huff0.Decoder.Decompress4X` in `decompress_asm.go` and
`decompress_generic.go`), which stay unbound as G2's build-tag functions
do.

### Every newly bound edge

| Rule | compress right / wrong | testify right / wrong |
|---|---|---|
| A field of another file's struct (`receiver_field`) | 155 / 0 | |
| An element of such a field | 81 / 0 | |
| A method of an embedded struct | 107 / 0 | 238 / 0 |
| A receiver typed in the file (field, element, channel, `make`, another package's type) | 161 / 0 | 18 / 0 |
| A same-file method the file now types | 13 / 0 | 2 / 0 |
| A call on a returned value the chain now types (`m.On(..).Return(..)`) | | 163 / 0 |
| **All** | **517 / 0** | **421 / 0** |

Precision against SCIP is 1.000 on both, with no edge SCIP could not
judge. Eight edges go 10 bound are bound differently, and all eight were
wrong: a field of another package's type (`compressor *flate.Writer` in
`gzip`, `zlib`, and `zip/register.go`) had been read as the caller
package's type of that name, so `z.compressor.Write(p)` in
`gzip/gzip.go:233` was a self-call to `gzip`'s `Writer.Write`. Go 11 binds
all eight to `flate/deflate.go::Writer`'s methods, which SCIP confirms.
`zlib/writer.go::Writer.Flush` loses its only caller that way: the caller
was the wrong self-call. No other function loses a caller.

### Hand check

A seeded random sample (`random.Random(2026)`) of the new edges, 20 of
compress and 10 of testify, each checked against the declaration of its
receiver (in parentheses).

| # | Call (file:line) | Caller | Bound to | Right |
|---|---|---|---|---|
| 1 | `flate/fuzz_test.go:73` `fw.ResetDict(..)` (`fw := encs[..]`, `encs := make([]*Writer, ..)`) | `FuzzEncoding` | `flate/deflate.go::Writer.ResetDict` | yes |
| 2 | `flate/huffman_bit_writer.go:301` `offEnc.codes[i].len()` (`offEnc *huffmanEncoder`, `codes []hcode`) | `huffmanBitWriter.generateCodegen` | `flate/huffman_code.go::hcode.len` | yes |
| 3 | `flate/inflate_gen.go:1308` `dict.writeCopy(..)` (`dict := &f.dict`, `dict dictDecoder` in `inflate.go`) | `decompressor.huffmanGenericReader` | `flate/dict_decoder.go::dictDecoder.writeCopy` | yes |
| 4 | `flate/level5.go:190` `e.matchlenLong(..)` (`e *fastEncL5`, which embeds `fastGen`) | `fastEncL5.Encode` | `flate/fast_encoder.go::fastGen.matchlenLong` | yes |
| 5 | `flate/level6.go:151` `e.matchlen(..)` (`e *fastEncL6`, embeds `fastGen`) | `fastEncL6.Encode` | `flate/fast_encoder.go::fastGen.matchlen` | yes |
| 6 | `flate/token.go:189` `tok.literal()` (`for _, tok := range in`, `in []token`) | `tokens.indexTokens` | `flate/token.go::token.literal` | yes |
| 7 | `fse/compress.go:187` `s.bw.flush32()` (`s *Scratch`, `bw bitWriter`) | `Scratch.compress` | `fse/bitwriter.go::bitWriter.flush32` | yes |
| 8 | `huff0/decompress.go:1068` `br.advance(..)` (`br := &br[i]`, `var br [4]bitReaderBytes`) | `Decoder.decompress4X8bitExactly` | `huff0/bitreader.go::bitReaderBytes.advance` | yes |
| 9 | `huff0/decompress_generic.go:40` `br[3].init(..)` (`var br [4]bitReaderShifted`) | `Decoder.Decompress4X` | `huff0/bitreader.go::bitReaderShifted.init` | yes |
| 10 | `huff0/decompress_generic.go:107` `br[stream].peekBitsFast(..)` (same `br`) | `Decoder.Decompress4X` | `huff0/bitreader.go::bitReaderShifted.peekBitsFast` | yes |
| 11 | `lzw/aldus_test.go:520` `r.Read(..)` (`r := lzw.NewReader(..).(*lzw.Reader)` in `lzw_test`) | `BenchmarkDecodeAldus` | `lzw/reader.go::Reader.Read` | yes |
| 12 | `zstd/blockenc.go:626` `mlEnc.normalizeCount(..)` (`mlEnc := b.coders.mlEnc`, `mlEnc *fseEncoder`) | `blockEnc.encode` | `zstd/fse_encoder.go::fseEncoder.normalizeCount` | yes |
| 13 | `zstd/decoder.go:491` `d.current.crc.Reset()` (`current decoderState`, `crc *xxhash.Digest`) | `Decoder.nextBlock` | `zstd/internal/xxhash/xxhash.go::Digest.Reset` | yes |
| 14 | `zstd/decoder.go:587` `d.frame.crc.Write(..)` (`frame *frameDec`, `crc *xxhash.Digest`) | `Decoder.nextBlockSync` | `zstd/internal/xxhash/xxhash.go::Digest.Write` | yes |
| 15 | `zstd/enc_fast.go:220` `e.matchlen(..)` (`e *fastEncoder`, embeds `fastBase`) | `fastEncoder.Encode` | `zstd/enc_base.go::fastBase.matchlen` | yes |
| 16 | `zstd/fse_decoder.go:278` `v.addBits(..)` (`range s.dt[:tableSize]`, `dt [maxTablesize]decSymbol`) | `fseDecoder.transform` | `zstd/fse_decoder.go::decSymbol.addBits` | yes |
| 17 | `zstd/fuzz_test.go:265` `enc.MaxEncodedSize(..)` (`enc := encs[level]`, `var encs [..]*Encoder`) | `FuzzEncoding` | `zstd/encoder.go::Encoder.MaxEncodedSize` | yes |
| 18 | `zstd/seqdec.go:260` `br.remain()` (`br := s.br`, `br *bitReader`) | `sequenceDecs.decodeSync` | `zstd/bitreader.go::bitReader.remain` | yes |
| 19 | `zstd/seqdec_generic.go:133` `llTable[llState].newState()` (`llTable := s.litLengths.fse.dt[:..]`) | `sequenceDecs.decode` | `zstd/fse_decoder.go::decSymbol.newState` | yes |
| 20 | `zstd/seqdec_test.go:162` `s.litLengths.fse.mustReadFrom(..)` (`s := sequenceDecs{..}`, `fse *fseDecoder`) | `readDecoders` | `zstd/fse_decoder.go::fseDecoder.mustReadFrom` | yes |
| 21 | `mock/mock_test.go:374` `mockedService.On(..)` (`TestExampleImplementation` embeds `Mock`) | `Test_Mock_On_WithFuncArgMatcher` | `mock/mock.go::Mock.On` | yes |
| 22 | `mock/mock_test.go:418` `mockedService.On(..)` (same type) | `Test_Mock_On_WithMapArgMatcher` | `mock/mock.go::Mock.On` | yes |
| 23 | `mock/mock_test.go:975` `.Return(..)` on `mockedService.On(..)` (`On` returns `*Call`) | `Test_Mock_Return_Nothing` | `mock/mock.go::Call.Return` | yes |
| 24 | `mock/mock_test.go:1240` `mockedService.On(..)` (`new(TestExampleImplementation)`) | `Test_Mock_Return_NotBefore_Orphan_Call` | `mock/mock.go::Mock.On` | yes |
| 25 | `mock/mock_test.go:1684` `.Twice()` on `.Return(..)` (`*Call`) | `Test_Mock_AssertExpectations_With_Repeatability` | `mock/mock.go::Call.Twice` | yes |
| 26 | `mock/mock_test.go:1692` `mockedService.AssertExpectations(t)` (embeds `Mock`) | the same test | `mock/mock.go::Mock.AssertExpectations` | yes |
| 27 | `mock/mock_test.go:1765` `.Return(..)` on `.On(..)` | `Test_Mock_AssertCalled_WithAnythingOfTypeArgument` | `mock/mock.go::Call.Return` | yes |
| 28 | `mock/mock_test.go:1817` `mockedService.AssertNotCalled(..)` (embeds `Mock`) | `Test_Mock_AssertNotCalled` | `mock/mock.go::Mock.AssertNotCalled` | yes |
| 29 | `mock/mock_test.go:1862` `.Maybe()` on `.Return(..)` | `Test_Mock_AssertOptional` | `mock/mock.go::Call.Maybe` | yes |
| 30 | `mock/mock_test.go:2118` `.WaitUntil(..)` on `.Return()` | `Test_WaitUntil_Parallel` | `mock/mock.go::Call.WaitUntil` | yes |

30 of 30 right, as SCIP says of all 938.

### The no-caller share after G2b

| | compress | testify |
|---|---|---|
| Production functions | 1,345 | 817 |
| No caller, go 10 | 419 (0.312) | 270 (0.330) |
| No caller, go 11 | **346 (0.257)** | **242 (0.296)** |
| go 11 with the scip-go overlay (`dagayn build --scip`) | 277 (0.206) | 149 (0.182) |
| Of go 11's, no direct reference in any SCIP build | 227 (0.169 of all) | 23 (0.028 of all) |

Test reach moves with it: compress's functions that ran but are called
untested go from 360 to 274 (untested precision 0.304 to 0.363, recall
0.913 to 0.907, tested precision 0.976 to 0.978), testify's from 42 to 32
(0.913 to 0.932, 0.995 to 0.993, 0.992 to 0.989).

### Why 0.15 cannot be reached this way

The clause counts a function with no caller whatever the reason, and on
compress 227 of the 346 left have no direct reference in any of the three
SCIP builds: interface dispatch, method values, `init`, `main`, and
functions nothing calls. Their share alone is 0.169, above the bar. Even
compiler-grade resolution of every direct reference does not reach 0.15:
the scip-go overlay on top of go 11 leaves 0.206, because it settles
direct references only. The 119 that SCIP references and the graph still
gives no caller are calls into other packages (30 functions, G3),
functions declared under two build tags (53), functions used as values
(13), and calls on values (23, most of them methods on what a call into
another package returned, or methods declared under two build tags).
testify is the other way round: 214 of its 242 are called only through a
package qualifier (mostly `assert` functions `require` calls as
`assert.Errorf`), which G3 binds, and only 23 have no direct reference.

Proposed revision, for the owner: measure the clause on the functions the
graph could give a caller to, those scip-go references directly. Of
compress's 1,046 such functions the graph gives no caller to 193 (0.185)
at go 10, 119 (0.114) at go 11, and 50 (0.048) with the scip-go overlay.
A bar of < 0.12 for this item is met. Binding the 30 cross-package
functions and the methods on cross-package results would bring it to
about 0.08 (an estimate from the counts above, not a measurement), a bar
to set for G3. The share over all functions stays
reported, with 0.169 as its floor on compress for anything that binds
direct calls and 0.15 reachable only with dispatch edges (an interface
method call bound to its implementations), which no item of the plan
builds.

## G3: imports bound to packages

<!-- derived-from #the-no-caller-share-after-g2b -->

Go extractor 12. An import of a package of the importing file's own module
(the nearest `go.mod`) targets the package's files: one `IMPORTS_FROM` edge
to each `.go` file of its directory, without its `_test.go` files and the
files the build ignores, each with `import_path` and `package_import`. An
external test package (`package kong_test`) imports the package it tests
the same way. A call through the import (`kong.New(...)`, which records
`import_path` since G2) binds in post-processing to the one top-level
function or type of that name whose file declares the import path as its
namespace and is not a test file; two under different build tags bind
nothing, as G2's plain calls do. A package import makes no file visible to
bare names, so G2's plain calls bind exactly as before. `importers_of`
takes a directory and reports its files' importers.

The before graphs are go 11 (main at `2fe1c685`), the after graphs go 12,
both full builds of copies of the corpora above, checked in the metadata.

### The bar

| Clause | Before (go 11) | After (go 12) | Met |
|---|---|---|---|
| in-module imports that reach a file: kong / testify / scip-go / compress | 0 of 12 / 24 / 48 / 127 | 12 / 24 / 48 / 127 (300 / 161 / 87 / 1,109 file edges) | yes |
| `importers_of internal/symbols` on scip-go | `not_found` | 8 importers | yes |
| `tests_for kong.go::New`: a nearest test | `options_test.go::TestOptions` (9 `TESTED_BY`, all from the package's own `options_test.go`) | `kong_test.go::mustNew` (65 `TESTED_BY` from `kong_test.go`, `mapper_test.go`, `options_test.go`, `tag_test.go`) | yes |
| review `dangling_rename_go` | fails (the call stays bare) | passes | yes |
| architecture `neg_go_test_package_imports_its_importer` | passes | passes | yes |

An in-module import is a distinct (importing file, import path) pair whose
path is the importing file's module path or below it, as the plan counted
it. scip-go's test data has 4 more in modules of their own under
`internal/testdata`, which reach files too (52 by the nearest `go.mod`).
compress has 6 imports from its nested modules (`zstd/_generate`,
`s2/cmd/_s2sx`) into the outer module through a `replace` directive; they
are another module's and keep their import path as the target, while the
calls through them bind (`s2/cmd/_s2sx/_unpack/main.go:69`
`s2.NewReader(rd)` to `s2/reader.go::NewReader`), since the called
package's files declare the import path.

`kong.go::New` had a nearest test at go 11 already, unlike the plan's
go 8 baseline: `options_test.go` is in `package kong` and calls `New`
plainly, which G2 binds. G3 adds the external test package's calls
(`kong.New(...)`, `kong_test.go:26`): calls bound to `New` go from 12 to 68.
The negative architecture case is package `b` importing `a` while `a`'s
external test package, in `a`'s directory, imports `b`. That is legal Go and
a cycle only if `a/a_test.go` were a target of `b`'s import, which the
extractor leaves out (a parser unit test checks it). The case passes on
main too, where no Go import reaches a file at all. Go forbids import cycles,
so no positive case exists. `import_cycles` reports none on the four
corpora at any `scope_kind`.

### Test reach and the no-caller share

| Repository | Ran, called untested (go 11 → 12) | `no_callers` / `no_test_path` / `beyond_limit` (ran, go 12) | Untested precision | Recall | Tested precision |
|---|---|---|---|---|---|
| testify | 32 → **21** of 292 | 17 / 4 / 0 | 0.932 → 0.954 | 0.993 → 0.991 | 0.989 → 0.985 (275) |
| compress | 274 → **237** of 978 | 144 / 89 / 4 | 0.363 → 0.381 | 0.907 → 0.849 | 0.978 → 0.966 (767) |
| scip-go | 100 → **51** of 110 | 4 / 39 / 8 | 0.213 → 0.338 | 1.000 → 0.963 | 1.000 → 0.983 (60) |

compress's recall drops because 10 functions that did not run now count as
tested: 9 of `internal/lz4ref`, which `s2`'s `TestLZ4Converter_*` call
through the import but which download their test data and failed offline
in the coverage run, and `zip/writer.go::Writer.RegisterCompressor`, whose
only test is an `Example` without an `Output:` comment, which `go test`
compiles and does not run. The edges are right (the SCIP check below); the
coverage run did not run those tests. testify and scip-go get one such
function each (`require.PanicsWithValue`, reached from `mock`'s tests,
which testify's coverage run leaves out). No function of the three lost
its test path.

| Production functions with no caller | compress | testify |
|---|---|---|
| all, go 11 | 346 of 1,345 (0.257) | 242 of 817 (0.296) |
| all, go 12 | **321 (0.239)** | **154 (0.188)** |
| of those scip-go references directly, go 11 | 119 of 1,046 (0.114) | 219 of 554 (0.395) |
| of those scip-go references directly, go 12 | **94 (0.090)** | **131 (0.236)** |

G2b's revised bar (under 0.12 over the directly referenced functions) holds
at 0.090 on compress, near its estimate of about 0.08 for G3. 125 of
testify's 131 are in `require`: `require_forward.go`'s `func (a
*Assertions) Equal(..)` calls `Equal(a.t, ..)`, the package's function, and
the extractor binds that plain call to the file's own method of the name.
That is a same-file binding left from before G3, not an import
([#236](https://github.com/manji-0/dagayn/issues/236)).

### Every newly bound edge

The script G2b used (`scipcheck.py`) joins each edge go 12 binds and go 11
did not to the scip-go reference of the method's name at the call, on the
same indexes (three builds of compress, two of testify).

| | compress right / wrong / unknown | testify right / wrong / unknown |
|---|---|---|
| Calls through an import | 319 / 0 / 9 | 642 / 0 / 7 |
| Calls on what such a call returned (`zstd.NewReader(..)` then `dec.Close()`) | 110 / 0 / 8 | |
| **All** | **429 / 0 / 17** | **642 / 0 / 7** |

Precision 1.000 on both. SCIP cannot judge 24 edges: 15 of compress's are
in `s2/cmd/_s2sx`, a module of its own that no index covers, and testify's
7 in `internal/spew`, which the indexes leave out; compress's other two are
conversions to a type of another package (`zstd.EncoderLevel(..)` in
`dict/cmd/builddict/main.go:41`, `lzw.Order(..)` in `lzw/aldus_test.go:425`),
right by hand. Edges go 11 bound that go 12 binds differently: 9 on
compress, all wrong before, and none on testify. They are `rc.Close()` on
the first named result of `func openFile(..) (rc io.ReadCloser, ..)` in
`s2/cmd/s2c` and `s2/cmd/s2d`, which the return-type pass read as the name
`ReadCloser` and bound to `zip/reader.go::ReadCloser.Close`. G3 made one
more of them (`readahead.NewReaderSize`, now bound, returns `(res
io.ReadCloser, err error)`), so the pass now reads a Go result's type
without its name, and all of them are `io.Closer`'s, as scip-go says.

kong and scip-go have no SCIP index here. kong gets 268 new edges (218
through an import), scip-go 89 (74).

### Hand check

A seeded sample (`random.Random(2026)`) of the new edges of each corpus, 6
of kong, 6 of scip-go, 8 of compress, 5 of testify, each checked against
the import of the caller's file and the declaration it binds to (the
receiver's declaration in parentheses).

| # | Call (file:line) | Caller | Bound to | Right |
|---|---|---|---|---|
| 1 | `help_test.go:304` `kong.Name("test-app")` | `TestHelpTree` | `options.go::Name` | yes |
| 2 | `help_test.go:681` `kong.Exit(func(int) {..})` | `TestHelpGrouping` | `options.go::Exit` | yes |
| 3 | `kong_test.go:2179` `kong.New(&cli)` | `TestDuplicateChildName` | `kong.go::New` | yes |
| 4 | `kong_test.go:2648` `parser.Parse(..)` (`parser, err := kong.New(..)`, `*Kong`) | `TestIssue483EmptyRootNodeNoRun` | `kong.go::Kong.Parse` | yes |
| 5 | `signature_test.go:76` `kong.Exit(..)` | `TestSignaturePointerReceiverHelp` | `options.go::Exit` | yes |
| 6 | `tag_test.go:145` `kong.Exit(..)` | `TestTagSetOnCommand` | `options.go::Exit` | yes |
| 7 | `internal/git/toplevel.go:12` `command.Run(dir, "git", ..)` | `TopLevel` | `internal/command/command.go::Run` | yes |
| 8 | `internal/implementations/implementations.go:43` `fingerprint.Encode(..)` | `methodMask` | `internal/implementations/fingerprint/fingerprint.go::Encode` | yes |
| 9 | `internal/lookup/lookup.go:114` `newtypes.GetID(pkg)` | `Global.GetPackage` | `internal/newtypes/newtypes.go::GetID` | yes |
| 10 | `internal/symbols/composer_test.go:13` `symbols.NewComposer("", "")` (`package symbols_test`) | `TestComposeNil` | `internal/symbols/composer.go::NewComposer` | yes |
| 11 | `internal/symbols/composer_test.go:16` `c.Compose(pkg, nil)` (`c := symbols.NewComposer(..)`, `*Composer`) | `TestComposeNil` | `internal/symbols/composer.go::Composer.Compose` | yes |
| 12 | `internal/visitors/visitor_file.go:388` `document.IsDocDeprecated(..)` | `fileVisitor.isDeprecated` | `internal/document/document.go::IsDocDeprecated` | yes |
| 13 | `gzhttp/compress_test.go:2030` `zs.Write(body)` (`zs, _ := zstd.NewWriter(..)`, `*Encoder`) | `newTestHandler` | `zstd/encoder.go::Encoder.Write` | yes |
| 14 | `gzhttp/compress_test.go:2352` `dec.Close()` (`dec, err := zstd.NewReader(..)`, `*Decoder`) | `TestZstdRandomJitter` | `zstd/decoder.go::Decoder.Close` | yes |
| 15 | `s2/cmd/_s2sx/_unpack/main.go:69` `s2.NewReader(rd)` (another module, `replace` to `../../..`) | `main` | `s2/reader.go::NewReader` | yes |
| 16 | `s2/fuzz_test.go:16` `fuzz.AddFromZip(..)` | `FuzzEncodingBlocks` | `internal/fuzz/helpers.go::AddFromZip` | yes |
| 17 | `s2/fuzz_test.go:116` `snapref.Decode(decDst, comp)` | `FuzzEncodingBlocks` | `internal/snapref/decode.go::Decode` | yes |
| 18 | `s2/s2_test.go:2137` `zip.NewReader(..)` | `TestDataRoundtrips` | `zip/reader.go::NewReader` | yes |
| 19 | `zstd/example_test.go:33` `r.DecodeAll(delta, nil)` (`r, _ := zstd.NewReader(..)`, `*Decoder`) | `ExampleWithEncoderDictRaw` | `zstd/decoder.go::Decoder.DecodeAll` | yes |
| 20 | `zstd/snappy_test.go:73` `snappy.NewBufferedWriter(&comp)` (`snappy "…/internal/snapref"`) | `TestSnappy_ConvertXML` | `internal/snapref/encode.go::NewBufferedWriter` | yes |
| 21 | `mock/mock_test.go:880` `require.Equal(..)` | `Test_Mock_Return_Run_Out_Of_Order` | `require/require.go::Equal` | yes |
| 22 | `mock/mock_test.go:1802` `assert.True(..)` | `Test_Mock_AssertCalled_WithArguments_With_Repeatability` | `assert/assertions.go::True` | yes |
| 23 | `mock/mock_test.go:2093` `assert.Equal(t, 123, args.Int(1))` | `Test_Arguments_Int` | `assert/assertions.go::Equal` | yes |
| 24 | `require/require.go:1412` `assert.NoFileExistsf(..)` | `NoFileExistsf` | `assert/assertion_format.go::NoFileExistsf` | yes |
| 25 | `require/require.go:1570` `assert.NotEqualf(..)` | `NotEqualf` | `assert/assertion_format.go::NotEqualf` | yes |

25 of 25 right.
