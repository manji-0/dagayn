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
