"""Fixture for cross-language bridge detection tests."""

import ctypes
import os
import subprocess

import cffi


def run_with_string_arg():
    subprocess.run("./target/release/dagayn-core")


def run_with_list_arg():
    subprocess.run(["./target/release/dagayn-core", "build", "--repo", "."])


def run_popen():
    subprocess.Popen(["./scripts/generate.sh", "--out", "out/"])


def run_check_call():
    subprocess.check_call(["./tools/lint"])


def load_cdll():
    lib = ctypes.CDLL("./target/release/libdagayn.so")
    return lib


def load_cdll_loadlibrary():
    lib = ctypes.cdll.LoadLibrary("./libhelper.so")
    return lib


def load_windll():
    lib = ctypes.WinDLL("mylib.dll")
    return lib


def load_cffi():
    # Direct chained form — detectable without dataflow tracking
    lib = cffi.FFI().dlopen("./libfoo.so")
    return lib


def run_system():
    os.system("make build")


def run_dynamic(cmd):
    # Non-literal argument — should produce LOW confidence edge
    subprocess.run(cmd)


def run_popen_dynamic(args):
    # Dynamic list — should produce LOW confidence edge
    subprocess.Popen(args)
