use super::*;

#[test]
fn resolves_c_includes_to_repo_relative_files() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-include-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/net")).unwrap();
    std::fs::create_dir_all(repo_root.join("include/mylib")).unwrap();
    std::fs::write(repo_root.join("src/net/socket.h"), b"struct Socket {};\n").unwrap();
    std::fs::write(repo_root.join("include/mylib/api.h"), b"void api();\n").unwrap();

    let source = br#"#include <vector>
#include "socket.h"
#include "mylib/api.h"
#include "missing.h"

void run() {}
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/net/client.cpp", source);
    let includes: Vec<&str> = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM")
        .map(|edge| edge.target.as_str())
        .collect();
    assert_eq!(
        includes,
        vec![
            // A system header matches no repo file and keeps its literal name.
            "vector",
            // Sibling header, resolved against the including directory.
            "src/net/socket.h",
            // Found by walking up to the directory holding `include/`.
            "include/mylib/api.h",
            // Unresolvable includes stay as written.
            "missing.h",
        ]
    );
    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_perl_xs_as_c_for_python_parity() {
    let source = br#"#include "EXTERN.h"
#include "perl.h"
#include "XSUB.h"
#include <string.h>

typedef struct {
    int x;
    int y;
} Point;

static int
_add(int a, int b) {
    return a + b;
}

static double
compute_distance(int x1, int y1, int x2, int y2) {
    return _add(x1, x2);
}

MODULE = MyModule  PACKAGE = MyModule

int
add(a, b)
    int a
    int b
  CODE:
    RETVAL = _add(a, b);
  OUTPUT:
    RETVAL
"#;
    let (nodes, edges) = parse_rust_owned_file("MyModule.xs", source);

    assert!(
        nodes
            .iter()
            .any(|node| node.kind == "Class" && node.name == "Point")
    );
    assert!(
        nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "_add")
    );
    assert!(
        nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "compute_distance")
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "XSUB.h")
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target.ends_with("::_add"))
    );
}

#[test]
fn parses_c_header_as_c_for_python_parity() {
    let source = br#"#ifndef USER_H
#define USER_H
#include <stdint.h>

typedef struct {
    int id;
} User;

static inline int user_id(User *user) {
    return user->id;
}

#endif
"#;
    let (nodes, edges) = parse_rust_owned_file("include/user.h", source);

    assert!(
        nodes
            .iter()
            .any(|node| node.kind == "File" && node.language == "c")
    );
    assert!(
        nodes
            .iter()
            .any(|node| node.kind == "Class" && node.name == "User")
    );
    assert!(
        nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "user_id")
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "stdint.h")
    );
}

#[test]
fn parses_c_structs_functions_imports_calls_and_bridges() {
    let source = br#"#include <stdio.h>
#include <dlfcn.h>

typedef struct {
    int id;
} User;

User* create_user(void) {
    return malloc(sizeof(User));
}

void print_user(User* user) {
    printf("%d", user->id);
}

void run_command(const char *cmd) {
    system("git status");
    fopen("config.yaml", "r");
    dlopen("mylib.so", RTLD_NOW);
    system(cmd);
}

int main() {
    User* u = create_user();
    print_user(u);
    return 0;
}
"#;
    let (nodes, edges) = parse_c("sample.c", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.language == "c"
            && node.extra["type_role"] == "class"
    }));
    assert!(
        nodes
            .iter()
            .any(|node| { node.kind == "Function" && node.name == "create_user" })
    );
    assert!(
        edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "stdio.h" })
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.c::main"
            && edge.target == "sample.c::create_user"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "sample.c::run_command"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "system"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "config.yaml"
            && edge.extra["relationship_role"] == "opens_file"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.so"
            && edge.extra["bridge_kind"] == "ffi"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:system@sample.c:20>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
}

#[test]
fn parses_cpp_classes_inheritance_functions_imports_calls_and_bridges() {
    let source = br#"#include <iostream>
#include <cstdlib>

class Animal {
public:
    Animal() {}
};

class Dog : public Animal {
public:
    Dog() : Animal() {}
    void speak() {}
};

void greet(const Animal& animal) {}

int main() {
    Dog d;
    d.speak();
    greet(d);
    return 0;
}

void run_command() {
    std::system("git status");
}

Animal *make_animal(int n) { return nullptr; }

void Dog::extra() { make_animal(1); }
"#;
    let (nodes, edges) = parse_cpp("sample.cpp", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Animal"
            && node.language == "cpp"
            && node.extra["type_role"] == "class"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function" && node.name == "Dog" && node.parent_name.as_deref() == Some("Dog")
    }));
    assert!(
        edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "iostream" })
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS" && edge.source == "sample.cpp::Dog" && edge.target == "Animal"
    }));
    // In-class members are indexed under their class, so `d.speak()` resolves.
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "speak"
            && node.parent_name.as_deref() == Some("Dog")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.cpp::main"
            && edge.target == "sample.cpp::Dog.speak"
    }));
    // A pointer return type wraps the declarator; the name is still the function.
    assert!(nodes.iter().any(|node| {
        node.kind == "Function" && node.name == "make_animal" && node.parent_name.is_none()
    }));
    // An out-of-line definition belongs to the class named by its scope.
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "extra"
            && node.parent_name.as_deref() == Some("Dog")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.cpp::Dog.extra"
            && edge.target == "sample.cpp::make_animal"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.cpp::main"
            && edge.target == "sample.cpp::greet"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "sample.cpp::run_command"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "std::system"
            && edge.extra["source_language"] == "cpp"
    }));
}

#[test]
fn parses_objc_classes_methods_imports_messages_and_c_functions() {
    let source = br#"#import <Foundation/Foundation.h>
#import "Logger.h"

@interface Calculator : NSObject
- (NSInteger)add:(NSInteger)a to:(NSInteger)b;
@end

@implementation Calculator

- (NSInteger)add:(NSInteger)a to:(NSInteger)b {
    NSInteger sum = a + b;
    [self logResult:sum];
    return sum;
}

- (void)logResult:(NSInteger)value {
    NSLog(@"Result: %ld", (long)value);
}

+ (Calculator *)sharedCalculator {
    return [[Calculator alloc] init];
}

@end

int main(int argc, const char * argv[]) {
    Calculator *calc = [Calculator sharedCalculator];
    NSInteger r = [calc add:3 to:4];
    NSLog(@"Final: %ld", (long)r);
    return 0;
}
"#;
    let (nodes, edges) = parse_objc("sample.m", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "Calculator" && node.language == "objc"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "add"
            && node.parent_name.as_deref() == Some("Calculator")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function" && node.name == "main" && node.parent_name.is_none()
    }));
    assert!(
        edges.iter().any(|edge| {
            edge.kind == "IMPORTS_FROM" && edge.target == "Foundation/Foundation.h"
        })
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.m::Calculator.add"
            && edge.target == "sample.m::Calculator.logResult"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.m::main"
            && edge.target == "sample.m::Calculator.sharedCalculator"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.m::main"
            && edge.target == "sample.m::Calculator.add"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.m::main" && edge.target == "NSLog"
    }));
}

#[test]
fn records_c_linkage_functions_as_ffi_exports() {
    let export = |nodes: &[crate::core::types::ParsedNode], name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .get("ffi_export")
            .cloned()
    };
    let c_symbol =
        |name: &str| Some(serde_json::json!({"abi": "c", "kind": "function", "name": name}));

    let c_source = br#"static double kahan(const double *xs, int n) { return 0; }
double fast_sum(const double *xs, int n) { return kahan(xs, n); }
__attribute__((visibility("hidden"))) int helper(void) { return 1; }
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("native/sum.c", c_source);
    assert_eq!(export(&nodes, "fast_sum"), c_symbol("fast_sum"));
    assert_eq!(export(&nodes, "kahan"), None);
    assert_eq!(export(&nodes, "helper"), None);

    let cpp_source = br#"namespace impl { double kahan(const double *xs, int n) { return 0; } }
extern "C" {
double fast_sum(const double *xs, int n) { return impl::kahan(xs, n); }
static int hidden_helper() { return 0; }
}
extern "C" int version() { return 1; }
int mangled() { return 0; }
class Widget { public: void draw() {} };
"#;
    let (nodes, _) = parser.parse_file("native/sum.cpp", cpp_source);
    assert_eq!(export(&nodes, "fast_sum"), c_symbol("fast_sum"));
    assert_eq!(export(&nodes, "version"), c_symbol("version"));
    assert_eq!(export(&nodes, "kahan"), None);
    assert_eq!(export(&nodes, "hidden_helper"), None);
    assert_eq!(export(&nodes, "mangled"), None);
    assert_eq!(export(&nodes, "draw"), None);
}
