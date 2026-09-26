"""Regression tests for per-language parse misses."""

from pathlib import Path

import pytest

from dagayn.parser import CodeParser


def _parse(tmp_path: Path, name: str, source: str):
    path = tmp_path / name
    path.write_text(source)
    nodes, edges = CodeParser().parse_file(path)
    prefix = str(path)

    def short(value: str) -> str:
        return value.replace(prefix, "<f>")

    names = {(n.kind, short(n.name), n.parent_name) for n in nodes}
    edge_set = {(e.kind, short(e.source), short(e.target)) for e in edges}
    return names, edge_set, nodes


@pytest.fixture
def parse(tmp_path):
    return lambda name, source: _parse(tmp_path, name, source)


class TestPython:
    SOURCE = """\
def helper(x):
    return x

class Svc:
    handler = lambda self, x: helper(x)
    marker = helper(0)

    def run(self):
        def local():
            helper(2)
        local()

    class Inner:
        def go(self):
            helper(3)
"""

    def test_nested_definitions_are_qualified(self, parse):
        names, edges, _ = parse("m.py", self.SOURCE)
        assert ("Function", "go", "Svc.Inner") in names
        assert ("Function", "local", "Svc.run") in names
        assert ("CONTAINS", "<f>::Svc", "<f>::Svc.Inner") in edges
        assert ("CONTAINS", "<f>::Svc.Inner", "<f>::Svc.Inner.go") in edges
        assert ("CONTAINS", "<f>::Svc.run", "<f>::Svc.run.local") in edges
        assert ("CALLS", "<f>::Svc.run", "<f>::Svc.run.local") in edges

    def test_class_body_calls_use_class_source(self, parse):
        _, edges, _ = parse("m.py", self.SOURCE)
        assert ("CALLS", "<f>::Svc", "<f>::helper") in edges

    def test_lambda_assignment_is_function(self, parse):
        names, edges, nodes = parse("m.py", self.SOURCE)
        assert ("Function", "handler", "Svc") in names
        assert ("CALLS", "<f>::Svc.handler", "<f>::helper") in edges
        handler = next(n for n in nodes if n.name == "handler")
        assert handler.extra.get("python_kind") == "lambda"


class TestRust:
    SOURCE = """\
use std::collections::{HashMap, HashSet as HS};
use crate::a::{run as ra, self};
fn top() {}
mod inner {
    pub fn deep() { super::top(); }
    pub mod sub { pub fn x() {} }
}
enum E { X }
impl E { fn go(&self) { Self::helper(); } fn helper() {} }
fn main() { inner::deep(); }
"""

    def test_grouped_use_is_split(self, parse):
        _, edges, _ = parse("lib.rs", self.SOURCE)
        imports = {t for k, _, t in edges if k == "IMPORTS_FROM"}
        assert imports == {
            "std::collections::HashMap",
            "std::collections::HashSet",
            "crate::a::run",
            "crate::a",
        }

    def test_inline_modules_scope_items(self, parse):
        names, edges, _ = parse("lib.rs", self.SOURCE)
        assert ("Class", "sub", "inner") in names
        assert ("Function", "x", "inner.sub") in names
        assert ("CONTAINS", "<f>::inner.sub", "<f>::inner.sub.x") in edges
        assert ("CALLS", "<f>::main", "<f>::inner.deep") in edges

    def test_self_and_super_paths_resolve(self, parse):
        _, edges, _ = parse("lib.rs", self.SOURCE)
        assert ("CALLS", "<f>::inner.deep", "<f>::top") in edges
        assert ("CALLS", "<f>::E.go", "<f>::E.helper") in edges


class TestGo:
    SOURCE = """\
package main

type Box[T any] struct{ v T }

func (b *Box[T]) Set(v T) { helper() }
func (Box[T]) Get() T { var z T; return z }

func helper() {}
"""

    def test_generic_receiver_uses_type_name(self, parse):
        names, edges, _ = parse("m.go", self.SOURCE)
        assert ("Function", "Set", "Box") in names
        assert ("Function", "Get", "Box") in names
        assert ("CONTAINS", "<f>::Box", "<f>::Box.Set") in edges

    def test_method_calls_use_qualified_caller(self, parse):
        _, edges, _ = parse("m.go", self.SOURCE)
        assert ("CALLS", "<f>::Box.Set", "<f>::helper") in edges


class TestJava:
    SOURCE = """\
public class Outer {
    static { init(); }
    public void run() {
        new Thread(new Runnable() { public void run() { inner(); } }).start();
    }
    record Point(int x, int y) { Point { check(x); } }
    class Inner { void m() { mcall(); } }
}
"""

    def test_nested_types_use_dotted_scope(self, parse):
        names, edges, _ = parse("Outer.java", self.SOURCE)
        assert ("Function", "m", "Outer.Inner") in names
        assert ("CONTAINS", "<f>::Outer.Inner", "<f>::Outer.Inner.m") in edges

    def test_initializer_calls_use_class_source(self, parse):
        _, edges, _ = parse("Outer.java", self.SOURCE)
        assert ("CALLS", "<f>::Outer", "init") in edges

    def test_anonymous_class_methods_do_not_collide(self, parse):
        names, edges, _ = parse("Outer.java", self.SOURCE)
        assert ("Function", "run", "Outer.run.Runnable") in names
        assert ("CALLS", "<f>::Outer.run.Runnable.run", "inner") in edges
        assert ("CALLS", "<f>::Outer.run", "inner") not in edges

    def test_compact_constructor(self, parse):
        names, edges, _ = parse("Outer.java", self.SOURCE)
        assert ("Function", "Point", "Outer.Point") in names
        assert ("CALLS", "<f>::Outer.Point.Point", "check") in edges


class TestCpp:
    SOURCE = """\
namespace ns { class A { void f(); }; }
void ns::A::f() {}
template <typename T> class V { void push(T); };
template <typename T> void V<T>::push(T x) { grow(); }
struct Outer { struct Inner { void g(); }; };
void Outer::Inner::g() { h(); }
class X : public ns::Base, public std::enable_shared_from_this<X>, private D {};
"""

    def test_all_base_classes_are_emitted(self, parse):
        _, edges, _ = parse("m.cpp", self.SOURCE)
        bases = {t for k, s, t in edges if k == "INHERITS" and s == "<f>::X"}
        assert bases == {"Base", "enable_shared_from_this", "D"}

    def test_out_of_line_scopes_match_class_paths(self, parse):
        names, edges, _ = parse("m.cpp", self.SOURCE)
        assert ("Function", "f", "A") in names
        assert ("Function", "push", "V") in names
        assert ("Class", "Inner", "Outer") in names
        assert ("Function", "g", "Outer.Inner") in names
        assert ("CALLS", "<f>::Outer.Inner.g", "h") in edges


class TestCSharp:
    SOURCE = """\
public class Foo {
    public void M() {
        int Local(int x) => Help(x);
        Local(1);
    }
    public class Inner { void I() { ICall(); } }
    public static Foo operator +(Foo a, Foo b) => Add(a, b);
    public static implicit operator int(Foo f) => 0;
    ~Foo() { Cleanup(); }
}
"""

    def test_nested_types_use_dotted_scope(self, parse):
        names, edges, _ = parse("Foo.cs", self.SOURCE)
        assert ("Function", "I", "Foo.Inner") in names
        assert ("CALLS", "<f>::Foo.Inner.I", "ICall") in edges

    def test_operators_and_destructors_are_members(self, parse):
        names, edges, _ = parse("Foo.cs", self.SOURCE)
        assert ("Function", "operator+", "Foo") in names
        assert ("Function", "operator int", "Foo") in names
        assert ("Function", "~Foo", "Foo") in names
        assert ("CALLS", "<f>::Foo.~Foo", "Cleanup") in edges

    def test_local_functions_belong_to_their_method(self, parse):
        names, edges, _ = parse("Foo.cs", self.SOURCE)
        assert ("Function", "Local", "Foo.M") in names
        assert ("CALLS", "<f>::Foo.M", "<f>::Foo.M.Local") in edges
        assert ("CALLS", "<f>::Foo.M.Local", "Help") in edges


class TestKotlin:
    SOURCE = """\
interface Iface { fun d() }
open class Base(a: Int)
class Foo(val a: Int) : Base(a), Iface {
    init { initCall() }
    constructor(s: String) : this(1) { secCall() }
    inner class In { fun im() { icall() } }
    override fun d() {}
}
object Singleton : Iface { fun s() { scall() } }
"""

    def test_supertypes_come_from_delegation_specifiers(self, parse):
        _, edges, _ = parse("m.kt", self.SOURCE)
        assert ("INHERITS", "<f>::Foo", "Base") in edges
        assert ("IMPLEMENTS", "<f>::Foo", "Iface") in edges
        assert ("INHERITS", "<f>::Foo", "Foo") not in edges

    def test_objects_and_interfaces_have_roles(self, parse):
        names, edges, nodes = parse("m.kt", self.SOURCE)
        roles = {n.name: n.extra.get("type_role") for n in nodes if n.kind == "Class"}
        assert roles["Singleton"] == "object"
        assert roles["Iface"] == "interface"
        assert ("CALLS", "<f>::Singleton.s", "scall") in edges

    def test_initializers_and_nested_classes(self, parse):
        names, edges, _ = parse("m.kt", self.SOURCE)
        assert ("CALLS", "<f>::Foo", "initCall") in edges
        assert ("CALLS", "<f>::Foo.constructor", "secCall") in edges
        assert ("Function", "im", "Foo.In") in names
        assert ("CONTAINS", "<f>::Foo", "<f>::Foo.In") in edges


class TestScala:
    SOURCE = """\
import x.y.{Z, W => V, H => _}
trait T
object O extends App with T {
  val f = (x: Int) => proc(x)
}
case class C(a: Int) extends Base(a) with T {
  object Nested { def n() = ncall() }
}
class K { def this(x: Int) = { this(); aux() } }
given intOrd: Ordering[Int] with { def compare(a: Int, b: Int) = cmp(a, b) }
"""

    def test_renamed_and_hidden_imports(self, parse):
        _, edges, _ = parse("m.scala", self.SOURCE)
        imports = {t for k, _, t in edges if k == "IMPORTS_FROM"}
        assert imports == {"x.y.Z", "x.y.W"}

    def test_object_supertypes_and_field_calls(self, parse):
        _, edges, _ = parse("m.scala", self.SOURCE)
        assert ("INHERITS", "<f>::O", "App") in edges
        assert ("IMPLEMENTS", "<f>::O", "T") in edges
        assert ("CALLS", "<f>::O", "proc") in edges

    def test_nested_objects_and_givens(self, parse):
        names, edges, _ = parse("m.scala", self.SOURCE)
        assert ("Function", "n", "C.Nested") in names
        assert ("CONTAINS", "<f>::C", "<f>::C.Nested") in edges
        assert ("Function", "compare", "intOrd") in names
        assert ("IMPLEMENTS", "<f>::intOrd", "Ordering") in edges

    def test_auxiliary_constructor_has_no_self_call(self, parse):
        _, edges, _ = parse("m.scala", self.SOURCE)
        assert ("CALLS", "<f>::K.this", "<f>::K.this") not in edges
        assert ("CALLS", "<f>::K.this", "aux") in edges


class TestSwift:
    SOURCE = """\
protocol Greeter { func greet() -> String }
struct Point: Equatable, Hashable {
    var x: Int
    var length: Int { return compute(x) }
    init(x: Int) { self.x = setup(x) }
    deinit { cleanup() }
    enum Dir { case up; func flip() -> Dir { return reverse() } }
}
class Foo: Bar, Greeter { func greet() -> String { "" } }
"""

    def test_every_inheritance_specifier(self, parse):
        _, edges, _ = parse("m.swift", self.SOURCE)
        assert ("INHERITS", "<f>::Foo", "Greeter") in edges
        assert ("INHERITS", "<f>::Point", "Hashable") in edges

    def test_initializers_and_computed_properties(self, parse):
        _, edges, _ = parse("m.swift", self.SOURCE)
        assert ("CALLS", "<f>::Point.init", "setup") in edges
        assert ("CALLS", "<f>::Point.deinit", "cleanup") in edges
        assert ("CALLS", "<f>::Point.length", "compute") in edges

    def test_nested_types_and_protocol_requirements(self, parse):
        names, _, _ = parse("m.swift", self.SOURCE)
        assert ("Function", "flip", "Point.Dir") in names
        assert ("Class", "Dir", "Point") in names
        assert ("Function", "greet", "Greeter") in names


class TestDart:
    SOURCE = """\
extension StrX on String {
  String shout() { return toUpperCase(); }
}
class A {
  final int x;
  A(this.x);
  A.named() : x = 0 { initNamed(); }
  factory A.make() { return A(build()); }
  int get twice => compute(x);
  set value(int v) { assign(v); }
}
void main() { A.make(); }
"""

    def test_constructors_getters_setters(self, parse):
        names, edges, _ = parse("m.dart", self.SOURCE)
        for member in ("A", "named", "make", "twice", "value"):
            assert ("Function", member, "A") in names
        assert ("CALLS", "<f>::A.named", "initNamed") in edges
        assert ("CALLS", "<f>::A.twice", "compute") in edges
        assert ("CALLS", "<f>::A.value", "assign") in edges

    def test_static_call_resolves_to_factory(self, parse):
        _, edges, _ = parse("m.dart", self.SOURCE)
        assert ("CALLS", "<f>::main", "<f>::A.make") in edges

    def test_extension_members(self, parse):
        names, _, nodes = parse("m.dart", self.SOURCE)
        assert ("Function", "shout", "StrX") in names
        ext = next(n for n in nodes if n.name == "StrX")
        assert ext.extra["type_role"] == "extension"


class TestPhp:
    SOURCE = """\
<?php
use App\\Models\\{User, Post as P};
interface Svc { public function run(); }
trait Loggable { public function log($m) { error_log($m); } }
class UserService extends Base implements Svc {
    use Loggable;
    public function run() { return new P(); }
}
"""

    def test_traits_are_types_with_members(self, parse):
        names, _, nodes = parse("m.php", self.SOURCE)
        assert ("Function", "log", "Loggable") in names
        trait = next(n for n in nodes if n.name == "Loggable")
        assert trait.extra["type_role"] == "trait"

    def test_extends_implements_and_trait_use(self, parse):
        _, edges, _ = parse("m.php", self.SOURCE)
        assert ("INHERITS", "<f>::UserService", "Base") in edges
        assert ("IMPLEMENTS", "<f>::UserService", "Svc") in edges
        assert ("INHERITS", "<f>::UserService", "Loggable") in edges

    def test_new_resolves_use_alias(self, parse):
        _, edges, _ = parse("m.php", self.SOURCE)
        assert ("CALLS", "<f>::UserService.run", "Post") in edges


class TestRuby:
    SOURCE = """\
require 'json'
module Outer
  class Inner < Base
    include Comparable
    attr_accessor :name
    def run
      process(1)
    end
  end
end
class Outer::Other < Lib::Base; end
"""

    def test_nested_scopes(self, parse):
        names, edges, nodes = parse("m.rb", self.SOURCE)
        assert ("Function", "run", "Outer.Inner") in names
        assert ("CONTAINS", "<f>::Outer", "<f>::Outer.Inner") in edges
        outer = next(n for n in nodes if n.name == "Outer")
        assert outer.extra["type_role"] == "module"

    def test_superclass_and_mixins(self, parse):
        names, edges, _ = parse("m.rb", self.SOURCE)
        assert ("INHERITS", "<f>::Outer.Inner", "Base") in edges
        assert ("INHERITS", "<f>::Outer.Inner", "Comparable") in edges
        assert ("Class", "Other", None) in names
        assert ("INHERITS", "<f>::Other", "Base") in edges

    def test_declarative_calls_are_not_calls(self, parse):
        _, edges, _ = parse("m.rb", self.SOURCE)
        targets = {t for k, _, t in edges if k == "CALLS"}
        assert not targets & {"require", "include", "attr_accessor"}


class TestPerl:
    SOURCE = """\
package Foo::Bar;
use strict;
use parent -norequire, 'Base::Thing';
use List::Util qw(max);
our @ISA = ('Other');
sub new { my $self = bless {}, shift; $self->init; return $self; }
sub init { max(1, 2); }
package Baz {
    sub run { init(); }
}
package main;
sub main_fn { Foo::Bar->new(); }
"""

    def test_subs_are_qualified_by_package(self, parse):
        names, _, _ = parse("m.pl", self.SOURCE)
        assert ("Function", "new", "Foo::Bar") in names
        assert ("Function", "run", "Baz") in names
        assert ("Function", "main_fn", None) in names

    def test_imports_and_inheritance(self, parse):
        _, edges, _ = parse("m.pl", self.SOURCE)
        imports = {t for k, _, t in edges if k == "IMPORTS_FROM"}
        assert imports == {"List::Util"}
        assert ("INHERITS", "<f>::Foo::Bar", "Base::Thing") in edges
        assert ("INHERITS", "<f>::Foo::Bar", "Other") in edges

    def test_class_method_calls_resolve(self, parse):
        _, edges, _ = parse("m.pl", self.SOURCE)
        assert ("CALLS", "<f>::main_fn", "<f>::Foo::Bar.new") in edges
        assert ("CALLS", "<f>::Foo::Bar.new", "<f>::Foo::Bar.init") in edges


class TestLua:
    SOURCE = """\
local M = {}
local function priv() M.f(1) end
function M.f(x) return x end
M.h = function(y) priv() end
local t = { cb = function() print("x") end }
function a.b.c() priv() end
"""

    def test_assigned_and_table_field_functions(self, parse):
        names, edges, _ = parse("m.lua", self.SOURCE)
        assert ("Function", "h", "M") in names
        assert ("Function", "cb", "t") in names
        assert ("CALLS", "<f>::M.h", "<f>::priv") in edges

    def test_nested_table_function_path(self, parse):
        names, edges, _ = parse("m.lua", self.SOURCE)
        assert ("Function", "c", "a.b") in names
        assert ("CALLS", "<f>::a.b.c", "<f>::priv") in edges

    def test_table_calls_resolve_through_table(self, parse):
        _, edges, _ = parse("m.lua", self.SOURCE)
        assert ("CALLS", "<f>::priv", "<f>::M.f") in edges


class TestR:
    SOURCE = """\
Person <- R6::R6Class("Person",
  inherit = Base,
  public = list(
    initialize = function(name) { helper() }
  )
)
setClass("S", contains = c("P", "Q"))
k <<- function() f()
(function(z) z) -> kk
f <- function() 1
"""

    def test_r6_classes(self, parse):
        names, edges, _ = parse("m.R", self.SOURCE)
        assert ("Function", "initialize", "Person") in names
        assert ("INHERITS", "<f>::Person", "Base") in edges
        assert ("CALLS", "<f>::Person.initialize", "helper") in edges

    def test_contains_inheritance(self, parse):
        _, edges, _ = parse("m.R", self.SOURCE)
        assert ("INHERITS", "<f>::S", "P") in edges
        assert ("INHERITS", "<f>::S", "Q") in edges

    def test_other_assignment_operators(self, parse):
        names, edges, _ = parse("m.R", self.SOURCE)
        assert ("Function", "k", None) in names
        assert ("Function", "kk", None) in names
        assert ("CALLS", "<f>::k", "<f>::f") in edges
