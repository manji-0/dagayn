"""End-to-end: Python -> Rust bridges through PyO3 (maturin) and ctypes."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from dagayn.graph import GraphStore
from dagayn.incremental import full_build
from dagayn.postprocessing import run_post_processing
from dagayn.tools.query import get_impact_radius
from tests.store_sql import store_conn

KAHAN = """
fn kahan(xs: &[f64]) -> f64 {
    xs.iter().sum()
}
"""


def _write(root: Path, files: dict[str, str]) -> None:
    for rel, text in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)


def _native_bridges(store: GraphStore) -> set[tuple[str, str, str]]:
    rows = (
        store_conn(store)
        .execute(
            "SELECT source_qualified, target_qualified, extra FROM edges "
            "WHERE kind = 'CROSS_ARTIFACT' "
            "AND json_extract(extra, '$.extractor') = 'native_bindings'"
        )
        .fetchall()
    )
    return {
        (
            row["source_qualified"],
            row["target_qualified"],
            json.loads(row["extra"])["relationship_role"],
        )
        for row in rows
    }


@pytest.fixture
def pyo3_repo(tmp_path: Path) -> Path:
    _write(
        tmp_path,
        {
            "pyproject.toml": (
                '[build-system]\nrequires = ["maturin>=1.9"]\nbuild-backend = "maturin"\n\n'
                '[project]\nname = "fastsum"\n\n'
                '[tool.maturin]\nmanifest-path = "rust/Cargo.toml"\nmodule-name = "fastsum._core"\n'
            ),
            "rust/Cargo.toml": (
                '[package]\nname = "fastsum-core"\nversion = "0.1.0"\n\n'
                '[lib]\nname = "_core"\ncrate-type = ["cdylib"]\n'
            ),
            "rust/src/lib.rs": (
                "use pyo3::prelude::*;\n\n"
                "#[pyfunction]\nfn fast_sum(xs: Vec<f64>) -> f64 {\n    kahan(&xs)\n}\n" + KAHAN
            ),
            "fastsum/__init__.py": (
                "from fastsum._core import fast_sum\n\n\n"
                "def total(values):\n    return fast_sum(list(values))\n"
            ),
            "fastsum/report.py": (
                "from fastsum import total\n\n\n"
                "def monthly_report(rows):\n    return total(r for r in rows)\n"
            ),
        },
    )
    return tmp_path


@pytest.fixture
def ctypes_repo(tmp_path: Path) -> Path:
    _write(
        tmp_path,
        {
            "native/Cargo.toml": (
                '[package]\nname = "fastsum"\nversion = "0.1.0"\n\n[lib]\ncrate-type = ["cdylib"]\n'
            ),
            "native/src/lib.rs": (
                "#[unsafe(no_mangle)]\n"
                'pub extern "C" fn fast_sum(ptr: *const f64, len: usize) -> f64 {\n'
                "    let xs = unsafe { std::slice::from_raw_parts(ptr, len) };\n    kahan(xs)\n}\n"
                + KAHAN
            ),
            "app/native.py": (
                "from ctypes import CDLL\n\n\n"
                'def load():\n    return CDLL("native/target/release/libfastsum.dylib")\n\n\n'
                "def total(values):\n"
                "    lib = load()\n"
                "    return lib.fast_sum(values, len(values))\n"
            ),
        },
    )
    return tmp_path


def _build(repo: Path) -> GraphStore:
    store = GraphStore(repo / ".dagayn" / "graph.db")
    full_build(repo, store)
    run_post_processing(store)
    return store


def test_pyo3_import_and_call_reach_the_rust_function(pyo3_repo: Path) -> None:
    store = _build(pyo3_repo)
    try:
        assert _native_bridges(store) == {
            ("fastsum/__init__.py", "rust/src/lib.rs", "loads_native_module"),
            ("fastsum/__init__.py::total", "rust/src/lib.rs::fast_sum", "calls_native_function"),
        }
        impact = get_impact_radius(
            changed_files=["rust/src/lib.rs"], repo_root=str(pyo3_repo), max_depth=2
        )
        assert "fastsum/report.py" in impact["impacted_files"]
    finally:
        store.close()


def test_ctypes_library_and_symbol_reach_the_rust_crate(ctypes_repo: Path) -> None:
    store = _build(ctypes_repo)
    try:
        assert _native_bridges(store) == {
            ("app/native.py::load", "native/src/lib.rs", "loads_shared_library"),
            ("app/native.py::total", "native/src/lib.rs::fast_sum", "calls_native_function"),
        }
        impact = get_impact_radius(
            changed_files=["native/src/lib.rs"], repo_root=str(ctypes_repo), max_depth=2
        )
        assert "app/native.py" in impact["impacted_files"]
    finally:
        store.close()


@pytest.fixture
def wasm_repo(tmp_path: Path) -> Path:
    _write(
        tmp_path,
        {
            "wasm/Cargo.toml": (
                '[package]\nname = "fast-sum"\nversion = "0.1.0"\n\n'
                '[lib]\ncrate-type = ["cdylib"]\n\n[dependencies]\nwasm-bindgen = "0.2"\n'
            ),
            "wasm/src/lib.rs": (
                "use wasm_bindgen::prelude::*;\n\n"
                "#[wasm_bindgen]\npub fn fast_sum(xs: &[f64]) -> f64 {\n    kahan(xs)\n}\n" + KAHAN
            ),
            "web/package.json": '{"name": "web", "dependencies": {"fast-sum": "file:../wasm/pkg"}}',
            "web/src/stats.ts": (
                'import init, { fast_sum } from "fast-sum";\n\n'
                "export async function total(values: Float64Array): Promise<number> {\n"
                "  await init();\n"
                "  return fast_sum(values);\n"
                "}\n"
            ),
            "web/src/report.ts": (
                'import { total } from "./stats";\n\n'
                "export async function monthlyReport(values: Float64Array) {\n"
                "  return { sum: await total(values) };\n"
                "}\n"
            ),
        },
    )
    return tmp_path


def test_typescript_import_and_call_reach_the_wasm_bindgen_function(wasm_repo: Path) -> None:
    store = _build(wasm_repo)
    try:
        assert _native_bridges(store) == {
            ("web/src/stats.ts", "wasm/src/lib.rs", "loads_native_module"),
            ("web/src/stats.ts::total", "wasm/src/lib.rs::fast_sum", "calls_native_function"),
        }
        impact = get_impact_radius(
            changed_files=["wasm/src/lib.rs"], repo_root=str(wasm_repo), max_depth=2
        )
        assert "web/src/report.ts" in impact["impacted_files"]
    finally:
        store.close()


@pytest.fixture
def go_wasm_repo(tmp_path: Path) -> Path:
    _write(
        tmp_path,
        {
            "gowasm/go.mod": "module example.com/gowasm\n\ngo 1.24\n",
            "gowasm/main.go": (
                "package main\n\n"
                "//go:wasmexport add\n"
                "func add(a, b int32) int32 {\n\treturn a + b\n}\n\n"
                "func main() {}\n"
            ),
            "web/package.json": (
                '{"scripts": {"wasm": '
                '"GOOS=wasip1 GOARCH=wasm go build -o public/add.wasm ../gowasm"}}'
            ),
            "web/src/add.ts": (
                "export async function addWith(a: number, b: number): Promise<number> {\n"
                "  const { instance } =\n"
                '    await WebAssembly.instantiateStreaming(fetch("/add.wasm"));\n'
                "  return (instance.exports as any).add(a, b);\n"
                "}\n"
            ),
            "web/src/report.ts": (
                'import { addWith } from "./add";\n\n'
                "export async function report() {\n"
                "  return addWith(1, 2);\n"
                "}\n"
            ),
        },
    )
    return tmp_path


def test_typescript_fetch_and_export_call_reach_the_go_function(go_wasm_repo: Path) -> None:
    store = _build(go_wasm_repo)
    try:
        assert _native_bridges(store) == {
            ("web/src/add.ts::addWith", "gowasm/main.go", "loads_native_module"),
            ("web/src/add.ts::addWith", "gowasm/main.go::add", "calls_native_function"),
        }
        impact = get_impact_radius(
            changed_files=["gowasm/main.go"], repo_root=str(go_wasm_repo), max_depth=2
        )
        assert "web/src/report.ts" in impact["impacted_files"]
    finally:
        store.close()


C_SUM = (
    "static double kahan(const double *xs, int n) {\n"
    "    double s = 0;\n    for (int i = 0; i < n; i++) s += xs[i];\n    return s;\n}\n\n"
    "double fast_sum(const double *xs, int n) {\n    return kahan(xs, n);\n}\n"
)
CTYPES_CALLER = (
    "import ctypes\n\n\n"
    "def load():\n    return ctypes.CDLL({library!r})\n\n\n"
    "def total(values):\n"
    "    lib = load()\n"
    "    return lib.fast_sum(values, len(values))\n\n\n"
    "def hidden(values):\n"
    "    return load().kahan(values, len(values))\n"
)
REPORT = "from app.native import total\n\n\ndef monthly_report(rows):\n    return total(rows)\n"


@pytest.mark.parametrize(
    ("build_files", "library"),
    [
        pytest.param(
            {
                "native/CMakeLists.txt": (
                    "cmake_minimum_required(VERSION 3.20)\nproject(fastsum C)\n"
                    "set(SOURCES src/sum.c)  # the library\n"
                    "add_library(fastsum SHARED ${SOURCES})\n"
                    "add_library(fastsum_static STATIC src/sum.c)\n"
                ),
            },
            "build/libfastsum.so",
            id="cmake",
        ),
        pytest.param(
            {
                "native/meson.build": (
                    "project('fastsum', 'c')\n"
                    "srcs = files('src/sum.c')\n"
                    "shared_library('fastsum', srcs, install: true)\n"
                ),
            },
            "libfastsum.dylib",
            id="meson",
        ),
        pytest.param(
            {
                "native/Makefile": (
                    "CFLAGS = -O2 -fPIC\n\n"
                    "libfastsum.so: src/sum.o\n"
                    "\t$(CC) $(CFLAGS) -shared -o $@ $^\n"
                ),
            },
            "native/libfastsum.so",
            id="make",
        ),
    ],
)
def test_ctypes_library_and_symbol_reach_the_c_source(
    tmp_path: Path, build_files: dict[str, str], library: str
) -> None:
    _write(
        tmp_path,
        {
            **build_files,
            "native/src/sum.c": C_SUM,
            "app/__init__.py": "",
            "app/native.py": CTYPES_CALLER.format(library=library),
            "app/report.py": REPORT,
        },
    )
    store = _build(tmp_path)
    try:
        # `kahan` is static: not a symbol ctypes can look up.
        assert _native_bridges(store) == {
            ("app/native.py::load", "native/src/sum.c", "loads_shared_library"),
            ("app/native.py::total", "native/src/sum.c::fast_sum", "calls_native_function"),
        }
        impact = get_impact_radius(
            changed_files=["native/src/sum.c"], repo_root=str(tmp_path), max_depth=2
        )
        assert "app/report.py" in impact["impacted_files"]
    finally:
        store.close()


def test_java_load_library_does_not_bind_calls_by_bare_name(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            "native/CMakeLists.txt": "add_library(fastsum SHARED src/sum.c)\n",
            "native/src/sum.c": C_SUM,
            "app/Sum.java": (
                "class Sum {\n"
                '    static { System.loadLibrary("fastsum"); }\n'
                "    static double fast_sum(double[] xs) { return 0; }\n"
                "    double total(double[] xs) { return fast_sum(xs); }\n"
                "}\n"
            ),
        },
    )
    store = _build(tmp_path)
    try:
        roles = {role for _, _, role in _native_bridges(store)}
        assert roles == {"loads_shared_library"}
    finally:
        store.close()


def test_csharp_p_invoke_reaches_the_c_function(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            "native/CMakeLists.txt": "add_library(fastsum SHARED src/sum.c)\n",
            "native/src/sum.c": C_SUM,
            "app/Native.cs": (
                "using System.Runtime.InteropServices;\n"
                "static class Native {\n"
                '    [DllImport("fastsum", EntryPoint = "fast_sum")]\n'
                "    static extern double FastSum(double[] xs, int n);\n"
                "    public static double Total(double[] xs) => FastSum(xs, xs.Length);\n"
                "}\n"
            ),
        },
    )
    store = _build(tmp_path)
    try:
        assert _native_bridges(store) == {
            ("app/Native.cs::Native.FastSum", "native/src/sum.c", "loads_shared_library"),
            (
                "app/Native.cs::Native.FastSum",
                "native/src/sum.c::fast_sum",
                "calls_native_function",
            ),
        }
        impact = get_impact_radius(
            changed_files=["native/src/sum.c"], repo_root=str(tmp_path), max_depth=3
        )
        impacted = {node["qualified_name"] for node in impact["impacted_nodes"]}
        assert "app/Native.cs::Native.Total" in impacted
    finally:
        store.close()


def test_java_and_kotlin_jni_reach_c_and_rust(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            "java/com/example/Sum.java": (
                "package com.example;\n"
                "public class Sum {\n"
                '    static { System.loadLibrary("fastsum"); }\n'
                "    public static native double fastSum(double[] xs);\n"
                "    public static native int overloaded(int x);\n"
                "    public static double total(double[] xs) { return fastSum(xs); }\n"
                "}\n"
            ),
            "java/com/example/Report.java": (
                "package com.example;\n"
                "class Report {\n"
                "    double monthly(double[] xs) { return Sum.total(xs); }\n"
                "}\n"
            ),
            "native/jni.c": (
                "#include <jni.h>\n"
                "JNIEXPORT jdouble JNICALL Java_com_example_Sum_fastSum("
                "JNIEnv *env, jclass cls, jdoubleArray xs) { return 0; }\n"
                "JNIEXPORT jint JNICALL Java_com_example_Sum_overloaded__I("
                "JNIEnv *env, jclass cls, jint x) { return x; }\n"
            ),
            "kotlin/com/example/Mean.kt": (
                "package com.example\n\n"
                "class Mean {\n    external fun mean(xs: DoubleArray): Double\n}\n"
            ),
            "rust/src/lib.rs": (
                "#[no_mangle]\n"
                'pub extern "system" fn Java_com_example_Mean_mean() -> f64 {\n    0.0\n}\n'
            ),
        },
    )
    store = _build(tmp_path)
    try:
        assert _native_bridges(store) == {
            (
                "java/com/example/Sum.java::Sum.fastSum",
                "native/jni.c::Java_com_example_Sum_fastSum",
                "calls_native_function",
            ),
            (
                "java/com/example/Sum.java::Sum.overloaded",
                "native/jni.c::Java_com_example_Sum_overloaded__I",
                "calls_native_function",
            ),
            (
                "kotlin/com/example/Mean.kt::Mean.mean",
                "rust/src/lib.rs::Java_com_example_Mean_mean",
                "calls_native_function",
            ),
        }
        impact = get_impact_radius(
            changed_files=["native/jni.c"], repo_root=str(tmp_path), max_depth=3
        )
        assert "java/com/example/Report.java" in impact["impacted_files"]
    finally:
        store.close()


NAPI_LIB = (
    "use napi_derive::napi;\n\n"
    "#[napi]\npub fn fast_sum(xs: Vec<f64>) -> f64 {\n    kahan(&xs)\n}\n" + KAHAN
)
NAPI_CARGO = (
    '[package]\nname = "fastsum"\nversion = "0.1.0"\n\n[lib]\ncrate-type = ["cdylib"]\n\n'
    '[dependencies]\nnapi = "3"\nnapi-derive = "3"\n'
)


def test_typescript_import_by_package_reaches_the_napi_function(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            "native/Cargo.toml": NAPI_CARGO,
            "native/src/lib.rs": NAPI_LIB,
            "native/package.json": (
                '{"name": "@demo/fastsum", "main": "index.js", "types": "index.d.ts",'
                ' "napi": {"binaryName": "fastsum"}}'
            ),
            "web/src/stats.ts": (
                'import { fastSum } from "@demo/fastsum";\n\n'
                "export function total(values: number[]): number {\n"
                "  return fastSum(values);\n"
                "}\n"
            ),
            "web/src/report.ts": (
                'import { total } from "./stats";\n\n'
                "export function monthlyReport(values: number[]) {\n"
                "  return { sum: total(values) };\n"
                "}\n"
            ),
        },
    )
    store = _build(tmp_path)
    try:
        bridges = _native_bridges(store)
        assert bridges == {
            ("web/src/stats.ts", "native/src/lib.rs", "loads_native_module"),
            ("web/src/stats.ts::total", "native/src/lib.rs::fast_sum", "calls_native_function"),
        }
        impact = get_impact_radius(
            changed_files=["native/src/lib.rs"], repo_root=str(tmp_path), max_depth=2
        )
        assert "web/src/report.ts" in impact["impacted_files"]
    finally:
        store.close()


def test_committed_napi_glue_at_the_repo_root_reaches_the_rust_function(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            "Cargo.toml": NAPI_CARGO,
            "src/lib.rs": NAPI_LIB,
            "package.json": '{"name": "fastsum", "main": "index.js", "types": "index.d.ts"}',
            "index.d.ts": "export declare function fastSum(xs: Array<number>): number\n",
            "index.js": (
                "const { fastSum } = require('./fastsum.darwin-arm64.node')\n"
                "module.exports.fastSum = fastSum\n"
            ),
            "lib/stats.ts": (
                'import { fastSum } from "../index";\n\n'
                "export function total(values: number[]): number {\n"
                "  return fastSum(values);\n"
                "}\n"
            ),
        },
    )
    store = _build(tmp_path)
    try:
        bridges = _native_bridges(store)
        assert ("lib/stats.ts::total", "src/lib.rs::fast_sum", "calls_native_function") in bridges
    finally:
        store.close()


def test_neon_require_of_the_built_addon_reaches_the_rust_function(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            "Cargo.toml": (
                '[package]\nname = "adder"\nversion = "0.1.0"\n\n[lib]\ncrate-type = ["cdylib"]\n\n'
                '[dependencies]\nneon = "1"\n'
            ),
            "src/lib.rs": (
                "#[neon::export]\nfn add_one(n: f64) -> f64 {\n    n + 1.0\n}\n\n"
                "fn legacy(mut cx: FunctionContext) -> JsResult<JsNumber> {\n    todo!()\n}\n\n"
                "#[neon::main]\nfn main(mut cx: ModuleContext) -> NeonResult<()> {\n"
                '    cx.export_function("legacy", legacy)?;\n    Ok(())\n}\n'
            ),
            "package.json": '{"name": "adder", "main": "index.node"}',
            "lib/math.js": (
                'const { addOne, legacy } = require("../index.node");\n\n'
                "function increment(n) {\n  return addOne(n);\n}\n\n"
                "function old() {\n  return legacy();\n}\n\n"
                "module.exports = { increment, old };\n"
            ),
        },
    )
    store = _build(tmp_path)
    try:
        assert _native_bridges(store) == {
            ("lib/math.js", "src/lib.rs", "loads_native_module"),
            ("lib/math.js::increment", "src/lib.rs::add_one", "calls_native_function"),
            ("lib/math.js::old", "src/lib.rs::legacy", "calls_native_function"),
        }
    finally:
        store.close()


GYP_ADDON_C = (
    "#include <node_api.h>\n"
    "static napi_value Hello(napi_env env, napi_callback_info info) { return NULL; }\n"
    "static napi_value Init(napi_env env, napi_value exports) {\n"
    "    napi_value fn;\n"
    '    napi_create_function(env, "hello", NAPI_AUTO_LENGTH, Hello, NULL, &fn);\n'
    "    return exports;\n}\n"
)


@pytest.mark.parametrize(
    "loader",
    [
        pytest.param("require('bindings')('greeter')", id="bindings"),
        pytest.param('require("../build/Release/greeter.node")', id="build-output"),
    ],
)
def test_node_gyp_addon_loads_and_calls_reach_the_c_function(tmp_path: Path, loader: str) -> None:
    _write(
        tmp_path,
        {
            "binding.gyp": (
                "# node-gyp build\n"
                "{\n  'targets': [\n    {\n      'target_name': 'greeter',\n"
                "      'sources': ['src/greeter.c'],\n    },\n  ],\n}\n"
            ),
            "package.json": '{"name": "greeter", "main": "lib/index.js", "gypfile": true}',
            "src/greeter.c": GYP_ADDON_C,
            "lib/index.js": (
                f"const addon = {loader};\n\n"
                "function greet() {\n  return addon.hello();\n}\n\n"
                "module.exports = { greet };\n"
            ),
        },
    )
    store = _build(tmp_path)
    try:
        bridges = _native_bridges(store)
        assert ("lib/index.js::greet", "src/greeter.c::Hello", "calls_native_function") in bridges
        assert any(
            target == "src/greeter.c" and role == "loads_native_module"
            for _, target, role in bridges
        )
        # A node-gyp addon is a Node.js module, not a ctypes library.
        assert not any(role == "loads_shared_library" for _, _, role in bridges)
    finally:
        store.close()


def test_wasm_imports_reach_the_javascript_that_implements_them(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            "wasm/Cargo.toml": (
                '[package]\nname = "clock"\nversion = "0.1.0"\n\n'
                '[lib]\ncrate-type = ["cdylib"]\n\n[dependencies]\nwasm-bindgen = "0.2"\n'
            ),
            "wasm/src/lib.rs": (
                "use wasm_bindgen::prelude::*;\n\n"
                '#[wasm_bindgen(module = "/js/util.js")]\n'
                'extern "C" {\n'
                "    #[wasm_bindgen(js_name = formatDate)]\n"
                "    fn format_date(ms: f64) -> String;\n"
                "}\n\n"
                "#[wasm_bindgen]\npub fn today(ms: f64) -> String {\n    format_date(ms)\n}\n"
            ),
            "wasm/js/util.js": (
                "export function formatDate(ms) {\n  return new Date(ms).toISOString();\n}\n"
            ),
            "gowasm/main.go": (
                "package main\n\n"
                "//go:wasmimport env log_value\n"
                "func logValue(v int32)\n\n"
                "//go:wasmexport run\n"
                "func run() {\n\tlogValue(1)\n}\n\n"
                "func main() {}\n"
            ),
            "web/src/host.ts": (
                "export const hostImports = {\n"
                "  env: {\n"
                "    log_value: (v: number) => console.log(v),\n"
                "  },\n"
                "};\n"
            ),
        },
    )
    store = _build(tmp_path)
    try:
        bridges = _native_bridges(store)
        assert (
            "wasm/src/lib.rs::format_date",
            "wasm/js/util.js::formatDate",
            "wraps_foreign_api",
        ) in bridges
        assert (
            "gowasm/main.go::logValue",
            "web/src/host.ts::hostImports.env.log_value",
            "wraps_foreign_api",
        ) in bridges
        # The imported declaration is not a Rust export JavaScript can call.
        assert not any(target == "wasm/src/lib.rs::format_date" for _, target, _ in bridges)
        impact = get_impact_radius(
            changed_files=["wasm/js/util.js"], repo_root=str(tmp_path), max_depth=3
        )
        impacted = {node["qualified_name"] for node in impact["impacted_nodes"]}
        assert "wasm/src/lib.rs::today" in impacted
    finally:
        store.close()


EM_SOURCE = (
    "#include <emscripten.h>\n"
    "static int twice(int x) { return x * 2; }\n"
    "EMSCRIPTEN_KEEPALIVE int add(int a, int b) { return twice(a) + b; }\n"
    "int sub(int a, int b) { return a - b; }\n"
    "int hidden(int a) { return a; }\n"
)


def test_emscripten_glue_and_ccall_reach_the_c_functions(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            "native/math.c": EM_SOURCE,
            "Makefile": (
                "web/dist/math.js: native/math.c\n"
                "\temcc $^ -O2 -o $@ -sMODULARIZE -sEXPORT_ES6 "
                "-sEXPORTED_FUNCTIONS=_add,_sub -sEXPORTED_RUNTIME_METHODS=ccall,cwrap\n"
            ),
            "web/src/calc.ts": (
                'import createModule from "../dist/math.js";\n\n'
                "export async function calc(a: number, b: number) {\n"
                "  const Module = await createModule();\n"
                "  const diff = Module.ccall('sub', 'number', ['number', 'number'], [a, b]);\n"
                "  return Module._add(a, b) + diff + Module._hidden(a);\n"
                "}\n"
            ),
        },
    )
    store = _build(tmp_path)
    try:
        bridges = _native_bridges(store)
        assert bridges == {
            ("web/src/calc.ts", "native/math.c", "loads_native_module"),
            ("web/src/calc.ts::calc", "native/math.c::add", "calls_native_function"),
            ("web/src/calc.ts::calc", "native/math.c::sub", "calls_native_function"),
        }
    finally:
        store.close()


def test_rust_extern_c_declarations_reach_the_c_they_link(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            # The crate compiles csrc/sum.c itself.
            "sys/Cargo.toml": '[package]\nname = "sys"\nversion = "0.1.0"\n',
            "sys/build.rs": (
                'fn main() {\n    cc::Build::new().file("csrc/sum.c").compile("sum");\n}\n'
            ),
            "sys/csrc/sum.c": "double fast_sum(const double *xs, int n) { return 0; }\n",
            "sys/src/lib.rs": (
                'extern "C" {\n    fn fast_sum(xs: *const f64, n: i32) -> f64;\n}\n\n'
                "pub fn total(xs: &[f64]) -> f64 {\n"
                "    unsafe { fast_sum(xs.as_ptr(), xs.len() as i32) }\n}\n"
            ),
            # Another C function of the same name elsewhere does not compete.
            "other/sum.c": "double fast_sum(const double *xs, int n) { return 1; }\n",
            # `#[link(name)]` picks the CMake library.
            "app/Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\n',
            "app/src/lib.rs": (
                '#[link(name = "mathx")]\nextern "C" {\n    fn scale(x: f64) -> f64;\n}\n\n'
                'extern "C" {\n    fn unique_helper() -> i32;\n'
                "    fn fast_sum(xs: *const f64, n: i32) -> f64;\n}\n"
            ),
            "mathx/CMakeLists.txt": "add_library(mathx SHARED scale.c)\n",
            "mathx/scale.c": "double scale(double x) { return x; }\n",
            "legacy/scale.c": "double scale(double x) { return -x; }\n",
            "helpers/helper.c": "int unique_helper(void) { return 1; }\n",
        },
    )
    store = _build(tmp_path)
    try:
        bridges = {
            (source, target)
            for source, target, role in _native_bridges(store)
            if role == "calls_native_function"
        }
        assert bridges == {
            ("sys/src/lib.rs::fast_sum", "sys/csrc/sum.c::fast_sum"),
            ("app/src/lib.rs::scale", "mathx/scale.c::scale"),
            ("app/src/lib.rs::unique_helper", "helpers/helper.c::unique_helper"),
            # app/src/lib.rs::fast_sum is ambiguous (two C definitions): unbound.
        }
        impact = get_impact_radius(
            changed_files=["sys/csrc/sum.c"], repo_root=str(tmp_path), max_depth=3
        )
        impacted = {node["qualified_name"] for node in impact["impacted_nodes"]}
        assert "sys/src/lib.rs::total" in impacted
    finally:
        store.close()


def test_cxx_bridge_links_rust_and_cpp_both_ways(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            "demo/Cargo.toml": '[package]\nname = "demo"\nversion = "0.1.0"\n',
            "demo/build.rs": (
                "fn main() {\n"
                '    cxx_build::bridge("src/main.rs")\n'
                '        .file("src/blobstore.cc")\n'
                '        .compile("cxxbridge-demo");\n'
                "}\n"
            ),
            "demo/src/main.rs": (
                '#[cxx::bridge(namespace = "org::blobstore")]\n'
                "mod ffi {\n"
                '    extern "Rust" {\n'
                "        type MultiBuf;\n"
                "        fn next_chunk(buf: &mut MultiBuf) -> &[u8];\n"
                "    }\n"
                '    unsafe extern "C++" {\n'
                '        include!("demo/include/blobstore.h");\n'
                "        type BlobstoreClient;\n"
                "        fn new_blobstore_client() -> UniquePtr<BlobstoreClient>;\n"
                "        fn put(self: Pin<&mut BlobstoreClient>, parts: &mut MultiBuf) -> u64;\n"
                "    }\n"
                "}\n\n"
                "pub struct MultiBuf { chunks: Vec<Vec<u8>>, pos: usize }\n\n"
                "pub fn next_chunk(buf: &mut MultiBuf) -> &[u8] {\n"
                "    let next = buf.chunks.get(buf.pos);\n    buf.pos += 1;\n"
                "    next.map_or(&[], Vec::as_slice)\n}\n\n"
                "fn main() {\n    let client = ffi::new_blobstore_client();\n}\n"
            ),
            "demo/src/blobstore.cc": (
                '#include "demo/include/blobstore.h"\n'
                "namespace org {\nnamespace blobstore {\n"
                "std::unique_ptr<BlobstoreClient> new_blobstore_client() {\n"
                "  return std::make_unique<BlobstoreClient>();\n}\n"
                "uint64_t BlobstoreClient::put(MultiBuf &buf) const {\n"
                "  while (true) {\n    auto chunk = next_chunk(buf);\n"
                "    if (chunk.size() == 0) break;\n  }\n  return 0;\n}\n"
                "}\n}\n"
            ),
            # Same-named C++ outside the crate's cxx_build sources.
            "other/impl.cc": "int new_blobstore_client() { return 0; }\n",
        },
    )
    store = _build(tmp_path)
    try:
        assert _native_bridges(store) == {
            (
                "demo/src/main.rs::ffi.new_blobstore_client",
                "demo/src/blobstore.cc::new_blobstore_client",
                "calls_native_function",
            ),
            (
                "demo/src/main.rs::ffi.put",
                "demo/src/blobstore.cc::BlobstoreClient.put",
                "calls_native_function",
            ),
            (
                "demo/src/blobstore.cc::BlobstoreClient.put",
                "demo/src/main.rs::next_chunk",
                "calls_native_function",
            ),
        }
    finally:
        store.close()


def test_cgo_calls_reach_c_and_c_calls_reach_go_exports(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            "fastsum/sum.go": (
                "package fastsum\n\n"
                '/*\n#cgo LDFLAGS: -lmathx\n#include "sum.h"\n*/\n'
                'import "C"\n\n'
                "func Total(n int) float64 {\n"
                "\treturn float64(C.fast_sum(C.int(n))) + float64(C.scale(1))\n}\n\n"
                "//export OnProgress\n"
                "func OnProgress(done C.int) {}\n"
            ),
            "fastsum/sum.c": (
                "void OnProgress(int done);\n"
                "double fast_sum(int n) {\n  OnProgress(n);\n  return 0;\n}\n"
            ),
            # Same symbol elsewhere: the package directory wins.
            "other/sum.c": "double fast_sum(int n) { return 1; }\n",
            # `scale` is found through `-lmathx`.
            "mathx/CMakeLists.txt": "add_library(mathx SHARED scale.c)\n",
            "mathx/scale.c": "double scale(double x) { return x; }\n",
            "legacy/scale.c": "double scale(double x) { return -x; }\n",
        },
    )
    store = _build(tmp_path)
    try:
        bridges = {
            (source, target)
            for source, target, role in _native_bridges(store)
            if role == "calls_native_function"
        }
        assert bridges == {
            ("fastsum/sum.go::Total", "fastsum/sum.c::fast_sum"),
            ("fastsum/sum.go::Total", "mathx/scale.c::scale"),
            ("fastsum/sum.c::fast_sum", "fastsum/sum.go::OnProgress"),
        }
    finally:
        store.close()


def test_python_calls_reach_pybind11_and_c_api_extensions(tmp_path: Path) -> None:
    _write(
        tmp_path,
        {
            "src/ext/bind.cpp": (
                "#include <pybind11/pybind11.h>\n"
                "namespace py = pybind11;\n"
                "int add(int a, int b) { return a + b; }\n"
                "struct Dog { void bark() {} };\n"
                "PYBIND11_MODULE(_core, m) {\n"
                '    m.def("add", &add);\n'
                '    py::class_<Dog>(m, "Dog");\n'
                "}\n"
            ),
            "src/ext/capi.c": (
                "#include <Python.h>\n"
                "static PyObject *py_scale(PyObject *self, PyObject *args) { return NULL; }\n"
                "static PyMethodDef Methods[] = {\n"
                '    {"scale", py_scale, METH_VARARGS, "Scale."},\n'
                "    {NULL, NULL, 0, NULL}\n};\n"
                "PyMODINIT_FUNC PyInit_capi(void) { return NULL; }\n"
            ),
            "pkg/__init__.py": "",
            "pkg/api.py": (
                "from pkg._core import add, Dog\n"
                "from pkg import capi\n\n\n"
                "def total(a, b):\n    return add(a, b)\n\n\n"
                "def pet():\n    return Dog()\n\n\n"
                "def scaled(x):\n    return capi.scale(x)\n"
            ),
        },
    )
    store = _build(tmp_path)
    try:
        bridges = _native_bridges(store)
        assert bridges == {
            ("pkg/api.py", "src/ext/bind.cpp", "loads_native_module"),
            ("pkg/api.py", "src/ext/capi.c", "loads_native_module"),
            ("pkg/api.py::total", "src/ext/bind.cpp::add", "calls_native_function"),
            ("pkg/api.py::pet", "src/ext/bind.cpp::Dog", "calls_native_function"),
            ("pkg/api.py::scaled", "src/ext/capi.c::py_scale", "calls_native_function"),
        }
    finally:
        store.close()
