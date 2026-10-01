use super::*;

#[test]
fn parses_perl_packages_subroutines_imports_calls_and_bridges() {
    let source = br#"use strict;
use warnings;
use File::Basename;

package Animal;

sub new {
    my ($class, %args) = @_;
    return bless \%args, $class;
}

sub speak {
    my ($self) = @_;
    return "...";
}

package Dog;

sub new {
    my ($class, %args) = @_;
    my $self = Animal::new($class, %args);
    return $self;
}

sub fetch {
    my ($self, $item) = @_;
    return "Fetched $item";
}

sub bark {
    my ($self) = @_;
    print $self->speak() . "\n";
}
"#;
    let (nodes, edges) = parse_perl("sample.pl", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Animal"
            && node.language == "perl"
            && node.extra["type_role"] == "class"
    }));
    assert!(
        nodes
            .iter()
            .any(|node| { node.kind == "Class" && node.name == "Dog" })
    );
    assert!(
        nodes
            .iter()
            .any(|node| { node.kind == "Function" && node.name == "bark" })
    );
    assert!(
        edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "File::Basename" })
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.pl::Animal.new"
            && edge.target == "CORE"
            && edge.extra["external_symbol"] == "bless"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.pl::Dog.bark"
            && edge.target == "sample.pl::Animal.speak"
    }));

    let bridge_source = br#"sub run_command {
    system("git status");
}

sub read_config {
    open(my $fh, '<', "config.yaml") or die;
    return $fh;
}

sub run_dynamic {
    my ($cmd) = @_;
    system($cmd);
}
"#;
    let (_nodes, bridge_edges) = parse_perl("bridge.pl", bridge_source);
    assert!(bridge_edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "system"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(bridge_edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:open@bridge.pl:6>"
            && edge.extra["evidence_source"] == "open"
            && edge.extra["confidence_tier"] == "LOW"
    }));
}

#[test]
fn perl_error_recovery_does_not_invent_callees() {
    let source = b"sub f {\n    return sub {\n        return input_avail && do {\n            $x = 1;\n        };\n    };\n}\n";
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("a.pl", source);
    let calls: Vec<&str> = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| edge.target.as_str())
        .collect();
    assert_eq!(calls, vec!["input_avail"]);
}

#[test]
fn perl_standard_library_calls_target_their_package() {
    let source = br#"use strict;
use POSIX qw(floor);
use Data::Dumper;
use List::Util ();
use My::Helpers qw(render);

sub run {
    my ($self, @items) = @_;
    my $n = floor(1.5);
    print Dumper(\@items);
    push @items, List::Util::max(@items);
    my $path = File::Spec->catfile('a', 'b');
    render(join(',', @items));
    $self->print($n);
    return sort_items(@items);
}

sub join { return "joined" }
"#;
    let (_, edges) = parse_perl("app.pl", source);
    let find = |target: &str, symbol: &str| {
        edges
            .iter()
            .find(|edge| {
                edge.target == target
                    && edge.extra["external_symbol"].as_str().unwrap_or_default() == symbol
            })
            .map(|edge| {
                (
                    edge.kind.as_str(),
                    edge.extra["confidence_tier"].as_str().unwrap_or_default(),
                )
            })
    };
    // A core module's `use` (a pragma makes no edge; a CPAN module is not
    // the standard library).
    assert_eq!(find("POSIX", ""), Some(("IMPORTS_FROM", "HIGH")));
    assert_eq!(find("Data::Dumper", ""), Some(("IMPORTS_FROM", "HIGH")));
    assert!(!edges.iter().any(|edge| edge.target == "strict"));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.target == "My::Helpers"
            && edge.extra.get("stdlib").is_none()
    }));
    // Imported by name or by default, or called through the module: certain.
    assert_eq!(find("POSIX", "POSIX::floor"), Some(("CALLS", "HIGH")));
    assert_eq!(
        find("Data::Dumper", "Data::Dumper::Dumper"),
        Some(("CALLS", "HIGH"))
    );
    assert_eq!(
        find("List::Util", "List::Util::max"),
        Some(("CALLS", "HIGH"))
    );
    assert_eq!(
        find("File::Spec", "File::Spec::catfile"),
        Some(("CALLS", "HIGH"))
    );
    // A builtin: likely.
    assert_eq!(find("CORE", "print"), Some(("CALLS", "MEDIUM")));
    assert_eq!(find("CORE", "push"), Some(("CALLS", "MEDIUM")));
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.extra.get("stdlib").is_none())
        .map(|edge| edge.target.as_str())
        .collect::<Vec<_>>();
    // A builtin the file declares, a repository import, a method of a
    // variable, and an unknown sub stay the repository's.
    for expected in ["app.pl::join", "render", "print", "sort_items"] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
}

#[test]
fn perl_receivers_record_the_call_they_came_from() {
    let source = br#"package Repo;
sub new { bless {}, shift }
sub save { }
package main;
sub run {
    my ($self, $p) = @_;
    my $s = Store->new($p);
    $s->save;
    my $r = Repo->new;
    $r->save;
    my $c = make_conn($p);
    $c->query(1);
    make_conn()->close();
    $self->helper();
    $p->save;
    $self->{db}->exec;
    $q->where(1)->where(2)->first;
}
"#;
    let (_, edges) = parse_perl("lib/app.pl", source);
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .unwrap_or_else(|| panic!("no {target} at {line} in {edges:?}"))
    };
    // `Store` is a package of another file.
    assert_eq!(call("save", 8).extra["receiver_type"], "Store");
    // `Repo` is this file's.
    assert!(
        call("lib/app.pl::Repo.save", 10)
            .extra
            .get("receiver_type")
            .is_none()
    );
    assert_eq!(
        call("query", 12).extra["receiver_from"],
        serde_json::json!({"call": "make_conn", "line": 11, "unwrap": false})
    );
    assert_eq!(
        call("close", 13).extra["receiver_from"],
        serde_json::json!({"call": "make_conn", "line": 13, "unwrap": false})
    );
    assert!(call("helper", 14).extra.get("receiver_unknown").is_none());
    // A parameter's method is not `Repo::save`.
    assert_eq!(call("save", 15).extra["receiver_unknown"], true);
    assert_eq!(call("exec", 16).extra["receiver_unknown"], true);
    // A method named `exec` runs no program.
    assert!(!edges.iter().any(|edge| edge.kind == "CROSS_ARTIFACT"));
    assert_eq!(call("first", 17).extra["receiver_from"]["call"], "where");
}
