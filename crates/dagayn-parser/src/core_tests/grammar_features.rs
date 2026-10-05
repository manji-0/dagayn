//! Holds in every feature set: a language whose `lang-*` feature is on is
//! parsed, and one whose feature is off still gets exactly its File node.

use super::*;

const NOTEBOOK: &str = r#"{"metadata": {"kernelspec": {"language": "python"}},
 "cells": [{"cell_type": "code", "source": ["def f():\n", "    pass\n"]}]}"#;

/// One small file per Rust-owned path kind, each declaring a symbol.
const SAMPLES: &[(&str, &str)] = &[
    ("a.md", "# Title\n\nText\n"),
    ("main.tf", "variable \"region\" {}\n"),
    ("a.rs", "fn f() {}\n"),
    ("a.py", "def f():\n    pass\n"),
    ("a.ipynb", NOTEBOOK),
    ("a.js", "function f() {}\n"),
    ("a.ts", "function f(): void {}\n"),
    ("a.tsx", "function f() { return <div />; }\n"),
    ("a.sh", "f() { echo hi; }\n"),
    ("a.go", "package p\nfunc f() {}\n"),
    ("A.java", "class A { void f() {} }\n"),
    ("a.rb", "def f\nend\n"),
    ("a.cs", "class A { void F() {} }\n"),
    ("a.php", "<?php\nfunction f() {}\n"),
    ("a.kt", "fun f() {}\n"),
    ("a.scala", "object O { def f() = 1 }\n"),
    ("a.dart", "void f() {}\n"),
    ("a.lua", "function f() end\n"),
    ("a.c", "int f(void) { return 0; }\n"),
    ("a.cpp", "int f() { return 0; }\n"),
    ("a.m", "@interface A : NSObject\n- (void)f;\n@end\n"),
    ("a.ex", "defmodule A do\n  def f, do: 1\nend\n"),
    ("a.gd", "func f():\n\tpass\n"),
    ("a.r", "f <- function() 1\n"),
    ("a.jl", "function f()\nend\n"),
    ("a.pl", "sub f { return 1; }\n"),
    (
        "a.vue",
        "<script>\nexport function f() {}\n</script>\n<template><div /></template>\n",
    ),
    (
        "a.svelte",
        "<script>\nfunction f() {}\n</script>\n<p>hi</p>\n",
    ),
    ("a.zig", "fn f() void {}\n"),
    ("a.swift", "func f() {}\n"),
    ("a.ps1", "function F {}\n"),
];

#[test]
fn disabled_languages_keep_only_their_file_node() {
    let mut kinds = HashSet::new();
    for (file_path, source) in SAMPLES {
        let kind = rust_owned_path_kind(file_path);
        assert_ne!(kind, RustOwnedPathKind::Unsupported, "{file_path}");
        kinds.insert(format!("{kind:?}"));

        let mut parser = RustOwnedParser::new();
        let (nodes, edges) = parser.parse_file(file_path, source.as_bytes());
        let files: Vec<_> = nodes
            .iter()
            .filter(|node| node.kind == NodeKind::File)
            .collect();
        assert_eq!(files.len(), 1, "{file_path}: {nodes:#?}");
        if kind != RustOwnedPathKind::Notebook || !kind.grammar_enabled() {
            // A parsed notebook's File node carries its kernel language.
            assert_eq!(files[0].language, kind.file_language(), "{file_path}");
        }

        if !kind.grammar_enabled() {
            assert_eq!(nodes.len(), 1, "{file_path}: {nodes:#?}");
            assert!(edges.is_empty(), "{file_path}: {edges:#?}");
        } else if kind != RustOwnedPathKind::PowerShell {
            assert!(nodes.len() > 1, "{file_path} parsed no symbol: {nodes:#?}");
        }
    }
    // Every kind but `Unsupported` has a sample.
    assert_eq!(kinds.len(), 31, "{kinds:?}");
}
