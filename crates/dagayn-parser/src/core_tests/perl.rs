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
        edge.kind == "CALLS" && edge.source == "sample.pl::Animal.new" && edge.target == "bless"
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
