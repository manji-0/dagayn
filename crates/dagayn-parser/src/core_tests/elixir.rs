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
        edge.kind == "CALLS"
            && edge.source == "sample.ex::Calculator.log"
            && edge.target == "IO"
            && edge.extra["external_symbol"] == "IO.puts"
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

#[test]
fn elixir_standard_library_calls_target_their_package() {
    let source = br#"defmodule Report do
  require Logger
  import Enum, only: [map: 2]
  alias MyApp.String
  alias Phoenix.Controller

  def build(rows) do
    Logger.info("building")
    names = map(rows, &name/1)
    :lists.reverse(names)
    String.upcase("x")
    Controller.json(rows)
    if is_nil(rows), do: raise("empty")
    inspect(names)
  end

  defp name(row), do: row.name
  defp inspect(value), do: value
end
"#;
    let (_, edges) = parse_elixir("report.ex", source);
    let tier = |kind: &str, target: &str, symbol: &str| {
        edges
            .iter()
            .find(|edge| {
                edge.kind == kind
                    && edge.target == target
                    && edge.extra["external_symbol"].as_str().unwrap_or_default() == symbol
            })
            .map(|edge| edge.extra["confidence_tier"].as_str().unwrap_or_default())
    };
    // Directives naming an Elixir module: certain; others are not stdlib.
    assert_eq!(tier("IMPORTS_FROM", "Logger", ""), Some("HIGH"));
    assert_eq!(tier("IMPORTS_FROM", "Enum", ""), Some("HIGH"));
    for target in ["MyApp.String", "Phoenix.Controller"] {
        assert!(edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM"
                && edge.target == target
                && edge.extra.get("stdlib").is_none()
        }));
    }
    // Remote calls into Elixir or OTP, and `import ..., only:`: certain.
    assert_eq!(tier("CALLS", "Logger", "Logger.info"), Some("HIGH"));
    assert_eq!(tier("CALLS", "Enum", "Enum.map"), Some("HIGH"));
    assert_eq!(tier("CALLS", ":lists", ":lists.reverse"), Some("HIGH"));
    // Kernel, bare: likely.
    assert_eq!(tier("CALLS", "Kernel", "is_nil"), Some("MEDIUM"));
    assert_eq!(tier("CALLS", "Kernel", "raise"), Some("MEDIUM"));
    // `String` aliased to the repository's, a third-party module, and a
    // Kernel name the module defines are not the standard library's.
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.extra.get("stdlib").is_none())
        .map(|edge| edge.target.as_str())
        .collect::<Vec<_>>();
    for expected in ["upcase", "json", "report.ex::Report.inspect"] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
}
