use super::*;

#[test]
fn parses_elixir_modules_functions_imports_and_calls() {
    let source = br#"defmodule Calculator do
  @moduledoc """
  Simple calculator module.
  """

  def add(a, b) do
    a + b
  end

  def subtract(a, b), do: a - b

  defp log(msg) do
    IO.puts(msg)
    :ok
  end

  def compute(a, b) do
    result = add(a, b)
    log("result: #{result}")
    result
  end
end

defmodule MathHelpers do
  alias Calculator
  import Calculator, only: [add: 2]
  require Logger

  def double(x) do
    Calculator.compute(x, x)
  end

  def triple(x) do
    double(x) + x
  end
end
"#;
    let (nodes, edges) = parse_elixir("sample.ex", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "Calculator" && node.language == "elixir"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "MathHelpers" && node.language == "elixir"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "add"
            && node.parent_name.as_deref() == Some("Calculator")
            && node.params.as_deref() == Some("(a, b)")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "log"
            && node.parent_name.as_deref() == Some("Calculator")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "triple"
            && node.parent_name.as_deref() == Some("MathHelpers")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "sample.ex" && edge.target == "Logger"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.ex::Calculator.compute"
            && edge.target == "sample.ex::Calculator.add"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.ex::Calculator.compute"
            && edge.target == "sample.ex::Calculator.log"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.ex::MathHelpers.double"
            && edge.target == "sample.ex::Calculator.compute"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.ex::MathHelpers.triple"
            && edge.target == "sample.ex::MathHelpers.double"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.ex" && edge.target == "moduledoc"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.ex::Calculator.log" && edge.target == "puts"
    }));
}

#[test]
fn elixir_anonymous_function_calls_are_not_edges() {
    let source = b"defmodule M do\n  def f(fun) do\n    fun.(1)\n    helper(2)\n  end\nend\n";
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("m.ex", source);
    let calls: Vec<&str> = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| edge.target.as_str())
        .collect();
    assert_eq!(calls, vec!["helper"]);
}
