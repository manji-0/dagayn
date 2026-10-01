use super::*;

#[test]
fn parses_ruby_classes_calls_imports_and_bridges() {
    let source = br#"require 'json'

module Auth
  class UserRepository
    def save(user)
      File.write("output.json", "{}")
      puts "Saved #{user}"
    end

    def create_user(name)
      save(name)
    end
  end
end

def run_command(path)
  system("git status")
  File.read(path)
  Fiddle.dlopen("mylib.so")
end
"#;
    let (nodes, edges) = parse_ruby("app.rb", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Auth"
            && node.parent_name.is_none()
            && node.language == "ruby"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "UserRepository"
            && node.parent_name.as_deref() == Some("Auth")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "save"
            && node.parent_name.as_deref() == Some("Auth.UserRepository")
            && node.params.is_none()
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "app.rb" && edge.target == "json"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "app.rb::Auth.UserRepository.create_user"
            && edge.target == "app.rb::Auth.UserRepository.save"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "output.json"
            && edge.extra["evidence_source"] == "File.write"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "system"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:File.read@app.rb:18>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.so"
            && edge.extra["evidence_source"] == "Fiddle.dlopen"
    }));
}

#[test]
fn ruby_calls_are_named_by_their_method() {
    let source = br#"def total(xs)
  Fast.fast_sum(xs, xs.size)
end
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("lib/total.rb", source);
    let mut calls: Vec<&str> = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| edge.target.as_str())
        .collect();
    calls.sort_unstable();
    assert_eq!(calls, vec!["fast_sum", "size"]);
}

#[test]
fn ruby_ffi_attach_function_defines_bound_module_methods() {
    let source = br#"require 'ffi'
module Fast
  extend FFI::Library
  ffi_lib 'libfastsum.so'
  attach_function :fast_sum, [:pointer, :int], :double
  attach_function :sum_alias, :fast_sum, [:pointer, :int], :double
end
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file("lib/fast.rb", source);
    let import = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .get("ffi_import")
            .cloned()
    };
    let expected = Some(serde_json::json!({
        "abi": "c", "name": "fast_sum", "library": "libfastsum.so"
    }));
    assert_eq!(import("fast_sum"), expected);
    assert_eq!(import("sum_alias"), expected);
    assert!(edges.iter().any(|edge| edge.kind == "CONTAINS"
        && edge.source == "lib/fast.rb::Fast"
        && edge.target == "lib/fast.rb::Fast.fast_sum"));
}

#[test]
fn ruby_calls_on_a_constant_record_the_receiver_type() {
    let source = b"def run(xs)\n  Fast.fast_sum(xs)\n  Outer::Inner.go\n  xs.size\nend\n";
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("lib/run.rb", source);
    let receiver = |target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target)
            .unwrap_or_else(|| panic!("no call {target}"))
            .extra
            .get("receiver_type")
            .cloned()
    };
    assert_eq!(receiver("fast_sum"), Some(serde_json::json!("Fast")));
    assert_eq!(receiver("go"), Some(serde_json::json!("Inner")));
    assert_eq!(receiver("size"), None);
}

#[test]
fn ruby_standard_library_calls_target_their_package() {
    let source = br#"require 'json'
require 'net/http'
require 'httparty'
require_relative 'set'

Point = Struct.new(:x)

def run(path, items)
  JSON.parse(File.read(path))
  Net::HTTP.get(URI(path))
  SecureRandom.hex(4)
  Set.new(items).include?(path)
  Point.new(1)
  HTTParty.get(path)
  puts "done"
  format("%s", path)
  items.puts
end

def format(*args)
  args.join
end
"#;
    let (_, edges) = parse_ruby("app.rb", source);
    let tier = |target: &str, symbol: &str| {
        edges
            .iter()
            .find(|edge| {
                edge.target == target
                    && edge.extra["external_symbol"].as_str().unwrap_or_default() == symbol
            })
            .map(|edge| {
                (
                    edge.kind.as_str().to_string(),
                    edge.extra["confidence_tier"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
    };
    let imports = |kind: &str, tier: &str| Some((kind.to_string(), tier.to_string()));
    // `require` of a library shipped with Ruby; never `require_relative`.
    assert_eq!(tier("json", ""), imports("IMPORTS_FROM", "HIGH"));
    assert_eq!(tier("net/http", ""), imports("IMPORTS_FROM", "HIGH"));
    assert!(
        edges
            .iter()
            .any(|edge| edge.target == "set" && edge.extra.get("stdlib").is_none())
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.target == "httparty" && edge.extra.get("stdlib").is_none())
    );
    // A constant of a required library: certain; not required: likely.
    assert_eq!(tier("json", "JSON.parse"), imports("CALLS", "HIGH"));
    assert_eq!(tier("net/http", "Net::HTTP.get"), imports("CALLS", "HIGH"));
    assert_eq!(
        tier("securerandom", "SecureRandom.hex"),
        imports("CALLS", "MEDIUM")
    );
    assert_eq!(tier("set", "Set.new.include?"), imports("CALLS", "MEDIUM"));
    // A core class: certain; a bare Kernel method: likely.
    assert_eq!(tier("core", "File.read"), imports("CALLS", "HIGH"));
    assert_eq!(tier("core", "puts"), imports("CALLS", "MEDIUM"));
    let call = |target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target)
            .unwrap_or_else(|| panic!("no call {target}"))
    };
    // The receiver type survives the rewrite.
    assert_eq!(call("json").extra["receiver_type"], "JSON");
    // A method the file defines, a constant it assigns, a gem, and a method
    // of a variable are not the standard library's.
    assert!(call("app.rb::format").extra.get("stdlib").is_none());
    assert!(call("new").extra.get("stdlib").is_none());
    assert!(call("get").extra.get("stdlib").is_none());
    assert!(call("puts").extra.get("stdlib").is_none());
}

#[test]
fn ruby_receivers_record_the_call_they_came_from() {
    let source = b"class Repo\n  def save\n  end\nend\n\ndef run(id)\n  store = Store.new(id)\n  store.save\n  repo = Repo.new\n  repo.save\n  user = find(id)\n  user.save\n  find(id).reload\n  q.where(1).where(2).first\n  @db.query\n  self.helper\nend\n";
    let (_, edges) = parse_ruby("lib/run.rb", source);
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .unwrap_or_else(|| panic!("no {target} at {line} in {edges:?}"))
    };
    // `Store` is a class of another file.
    assert_eq!(call("save", 8).extra["receiver_type"], "Store");
    assert!(call("save", 8).extra.get("receiver_unknown").is_none());
    // `Repo` is this file's, and defines `save`.
    assert_eq!(
        call("lib/run.rb::Repo.save", 10).extra.get("receiver_type"),
        None
    );
    assert_eq!(
        call("save", 12).extra["receiver_from"],
        serde_json::json!({"call": "find", "line": 11, "unwrap": false})
    );
    assert_eq!(call("save", 12).extra["receiver_unknown"], true);
    assert_eq!(
        call("reload", 13).extra["receiver_from"],
        serde_json::json!({"call": "find", "line": 13, "unwrap": false})
    );
    assert_eq!(call("where", 14).extra["receiver_unknown"], true);
    assert_eq!(call("where", 14).extra.get("receiver_from"), None);
    assert_eq!(call("first", 14).extra["receiver_from"]["call"], "where");
    assert_eq!(call("query", 15).extra["receiver_unknown"], true);
    assert!(call("helper", 16).extra.get("receiver_unknown").is_none());
}
