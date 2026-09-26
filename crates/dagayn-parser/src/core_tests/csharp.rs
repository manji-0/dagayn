use super::*;

#[test]
fn csharp_methods_keep_declared_names_and_resolve_qualified_calls() {
    let source = br#"internal static class CertificateCriteriaFactory
{
    internal static ClientCertificateInfo CreateAllowedCertificateCriteria(
        CertificateTypes certificateType)
    {
        return new ClientCertificateInfo(certificateType);
    }

    internal static List<Issuer> GetClientCertificateIssuers<T>(T source)
    {
        return null;
    }
}

public abstract class CertificateStoreBroker
{
    public ClientCertificateInfo Resolve(CertificateTypes certificateType)
    {
        return CertificateCriteriaFactory.CreateAllowedCertificateCriteria(certificateType);
    }
}
"#;
    let (nodes, edges) = parse_csharp("Certificate.cs", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "CreateAllowedCertificateCriteria"
            && node.parent_name.as_deref() == Some("CertificateCriteriaFactory")
            && node.return_type.as_deref() == Some("ClientCertificateInfo")
    }));
    // The generic return type must not leak into the method name either.
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "GetClientCertificateIssuers"
            && node.return_type.as_deref() == Some("List<Issuer>")
    }));
    assert!(nodes.iter().all(|node| {
        node.kind != "Function" || !matches!(node.name.as_str(), "ClientCertificateInfo" | "List")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "CertificateStoreBroker"
            && node.extra["type_role"] == "abstract_class"
            && node.extra["is_abstract"] == true
    }));
    // `Factory.Method(...)` resolves to the method, not the receiver type.
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "Certificate.cs::CertificateStoreBroker.Resolve"
            && edge.target
                == "Certificate.cs::CertificateCriteriaFactory.CreateAllowedCertificateCriteria"
    }));
    // `new Type(...)` records a call to the constructed type.
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source
                == "Certificate.cs::CertificateCriteriaFactory.CreateAllowedCertificateCriteria"
            && edge.target == "ClientCertificateInfo"
    }));
}

#[test]
fn parses_csharp_types_imports_and_bridges() {
    let source = br#"using System.IO;
using System.Diagnostics;
using System.Reflection;

struct User
{
    public string Path;
}

interface IRepository
{
    User FindById(int id);
    void Save(User user);
}

class BridgeSamples : IRepository
{
    public User FindById(int id)
    {
        return null;
    }

    public void Save(User user)
    {
        Process.Start("git", "status");
        File.ReadAllText(user.Path);
        Assembly.LoadFile("mylib.dll");
    }
}
"#;
    let (nodes, edges) = parse_csharp("sample.cs", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "IRepository"
            && node.extra["type_role"] == "interface"
            && node.extra["is_contract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "BridgeSamples" && node.extra["type_role"] == "class"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.extra["type_role"] == "struct"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "FindById"
            && node.parent_name.as_deref() == Some("BridgeSamples")
            && node.params.as_deref() == Some("(int id)")
            && node.return_type.as_deref() == Some("User")
    }));
    assert!(
        !nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "User")
    );
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "Save"
            && node.parent_name.as_deref() == Some("BridgeSamples")
            && node.params.as_deref() == Some("(User user)")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "sample.cs" && edge.target == "System.IO"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPLEMENTS"
            && edge.source == "sample.cs::BridgeSamples"
            && edge.target == "IRepository"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git"
            && edge.extra["evidence_source"] == "Process.Start"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:File.ReadAllText@sample.cs:26>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.dll"
            && edge.extra["evidence_source"] == "Assembly.LoadFile"
    }));
}

#[test]
fn parses_csharp_records_implements_inherits_and_properties() {
    let source = br#"
public interface IRepo
{
    User Find(int id);
}

public record Person(string Name);

public class Repo : IRepo
{
    public User Find(int id) { return null; }
}

public class Service : Repo
{
    public int Count { get; set; }

    public User Get(int id)
    {
        return Find(id);
    }
}
"#;
    let (nodes, edges) = parse_csharp("sample.cs", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Person"
            && node.extra["type_role"] == "record"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "Count"
            && node.parent_name.as_deref() == Some("Service")
            && node.extra["member_role"] == "property"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPLEMENTS" && edge.source == "sample.cs::Repo" && edge.target == "IRepo"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS" && edge.source == "sample.cs::Service" && edge.target == "Repo"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "Find"
            && node.parent_name.as_deref() == Some("IRepo")
            && node.extra["is_abstract"] == true
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.cs::Service.Get"
            && (edge.target == "sample.cs::IRepo.Find" || edge.target == "sample.cs::Repo.Find")
    }));
    assert!(!edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.cs::Service.Get"
            && edge.target == "sample.cs::Find"
    }));
}

#[test]
fn csharp_member_calls_bind_constructor_local_and_test_metadata() {
    let source = br#"
using RepoAlias = Repo;
using System.IO;

public interface IRepo
{
    User Find(int id);
}

public abstract class Base
{
    public abstract User Load();
}

public record struct Point(int X);

public class Outer
{
    public class Inner {}
}

public class Repo : IRepo
{
    public User this[int id] { get { return null; } }

    public User Find(int id) { return this.Ping(id); }

    public User Ping(int id) { return null; }
}

public class Service : Repo
{
    private Repo _repo = new Repo();

    public User FromBase(int id)
    {
        return base.Find(id);
    }

    public User Run(Repo repo)
    {
        var local = new Repo();
        Repo typed = new();
        local.Find(1);
        typed.Find(2);
        repo.Find(3);
        _repo.Find(4);
        new Repo().Find(5);
        return local.Find(6);
    }

    [Fact]
    public void FindsUser()
    {
        var repo = new Repo();
        repo.Find(1);
    }
}

// dagayn: implements docs/spec.md#Repo
"#;
    let (nodes, edges) = parse_csharp("sample.cs", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Type" && node.name == "RepoAlias" && node.extra["type_role"] == "alias"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "sample.cs" && edge.target == "Repo"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "sample.cs" && edge.target == "System.IO"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Point"
            && node.extra["type_role"] == "record"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "Load"
            && node.parent_name.as_deref() == Some("Base")
            && node.extra["is_abstract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "this"
            && node.parent_name.as_deref() == Some("Repo")
            && node.extra["member_role"] == "indexer"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "Inner" && node.parent_name.as_deref() == Some("Outer")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS"
            && edge.source == "sample.cs::Outer"
            && edge.target == "sample.cs::Outer.Inner"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Test"
            && node.name == "FindsUser"
            && node.parent_name.as_deref() == Some("Service")
            && node.is_test
            && node.extra["attributes"]
                .as_array()
                .is_some_and(|values| values.iter().any(|value| value == "Fact"))
    }));
    assert!(
        edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "sample.cs::Service.Run"
                && edge.target == "sample.cs::Repo.Find"
        }),
        "{edges:?}"
    );
    assert!(!edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.cs::Service.Run"
            && edge.target == "sample.cs::IRepo.Find"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.cs::Repo.Find"
            && edge.target == "sample.cs::Repo.Ping"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.cs::Service.FromBase"
            && edge.target == "sample.cs::Repo.Find"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.cs::Service.FindsUser"
            && edge.target == "sample.cs::Repo.Find"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "TESTED_BY"
            && edge.source == "sample.cs::Repo.Find"
            && edge.target == "sample.cs::Service.FindsUser"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.extra["evidence_kind"] == "comment_directive"
            && edge.extra["relationship_role"] == "implements_contract"
            && edge.target.contains("docs/spec.md")
    }));
}
