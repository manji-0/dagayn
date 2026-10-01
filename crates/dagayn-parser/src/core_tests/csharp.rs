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

#[test]
fn csharp_p_invoke_declarations_bind_their_c_symbols() {
    let source = br#"using System.Runtime.InteropServices;
static partial class Native {
    private const string Lib = "fastsum";
    [DllImport(Lib, EntryPoint = "fast_sum", CallingConvention = CallingConvention.Cdecl)]
    public static extern double FastSum(double[] xs, int n);
    [LibraryImport("libfastsum.so")]
    internal static partial int version();
    [DllImport("kernel32.dll")]
    static extern bool Beep(uint freq, uint ms);
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("app/Native.cs", source);
    let imports: Vec<(&str, &str, &str, &str)> = edges
        .iter()
        .filter(|edge| edge.kind == "CROSS_ARTIFACT")
        .map(|edge| {
            (
                edge.source.as_str(),
                edge.target.as_str(),
                edge.extra["symbol"].as_str().unwrap_or_default(),
                edge.extra["evidence_source"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        imports,
        vec![
            (
                "app/Native.cs::Native.FastSum",
                "fastsum",
                "fast_sum",
                "DllImport"
            ),
            (
                "app/Native.cs::Native.version",
                "libfastsum.so",
                "version",
                "LibraryImport"
            ),
            (
                "app/Native.cs::Native.Beep",
                "kernel32.dll",
                "Beep",
                "DllImport"
            ),
        ]
    );
}

#[test]
fn csharp_static_calls_record_the_receiver_type() {
    let source = br#"class Report {
    double Monthly(double[] xs) {
        var local = new Helper();
        local.Run();
        System.IO.File.ReadAllText("x");
        return Native.Total(xs) + service.Compute();
    }
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("app/Report.cs", source);
    let receiver = |target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target.ends_with(target))
            .unwrap_or_else(|| panic!("no call {target}"))
            .extra
            .get("receiver_type")
            .cloned()
    };
    assert_eq!(receiver("Total"), Some(serde_json::json!("Native")));
    // A call into the base class library targets its namespace, and keeps
    // the receiver type.
    let read = edges
        .iter()
        .find(|edge| edge.extra["external_symbol"] == "System.IO.File.ReadAllText")
        .expect("no call to File.ReadAllText");
    assert_eq!(read.target, "System.IO");
    assert_eq!(read.extra["receiver_type"], "File");
    assert_eq!(receiver("Compute"), None);
    // `var local = new Helper()`, `Helper` declared in another file.
    assert_eq!(receiver("Run"), Some(serde_json::json!("Helper")));
}

#[test]
fn csharp_variables_of_other_file_types_record_the_receiver_type() {
    let source = br#"class Report {
    double Monthly(double[] xs, Native param, string label) {
        var made = new Native();
        Native declared = Create();
        label.Trim();
        return made.Total(xs) + declared.Mean(xs) + param.Max(xs);
    }
    double Other(double[] xs) {
        return made.Total(xs);
    }
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("app/Report.cs", source);
    let receivers = |target: &str| -> Vec<Option<serde_json::Value>> {
        edges
            .iter()
            .filter(|edge| edge.kind == "CALLS" && edge.target == target)
            .map(|edge| edge.extra.get("receiver_type").cloned())
            .collect()
    };
    let native = Some(serde_json::json!("Native"));
    assert_eq!(receivers("Native"), vec![native.clone()]);
    assert_eq!(receivers("Mean"), vec![native.clone()]);
    assert_eq!(receivers("Max"), vec![native.clone()]);
    // `made` is out of scope in `Other`.
    assert_eq!(receivers("Total"), vec![native, None]);
    // `string` is not a type to match: its methods are the base class
    // library's.
    let trim = edges
        .iter()
        .find(|edge| edge.kind == "CALLS" && edge.extra["external_symbol"] == "string.Trim")
        .expect("no call to string.Trim");
    assert_eq!(trim.target, "System");
    assert_eq!(trim.extra.get("receiver_type"), None);
}

#[test]
fn csharp_standard_library_calls_target_their_package() {
    let source = br#"using System;
using System.Text;
using System.Collections.Generic;
using Acme.Utils;

class Path { public static string Combine(string a, string b) { return a; } }

class Report
{
    void Run(List<int> xs, Native native)
    {
        Console.WriteLine("x");
        string.Join(",", xs);
        System.IO.File.ReadAllText("x");
        var sb = new StringBuilder();
        sb.Append(1);
        new StringBuilder().Append(2);
        xs.Add(1);
        Math.Max(1, 2);
        int.Parse("1");
        Path.Combine("a", "b");
        native.Total();
        Helpers.Join(xs);
        File.ReadAllText("y");
    }
}
"#;
    let (_, edges) = parse_csharp("Report.cs", source);
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| {
            (
                edge.target.as_str(),
                edge.extra["external_symbol"].as_str().unwrap_or_default(),
                edge.extra["confidence_tier"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        ("System", "Console.WriteLine", "HIGH"),
        ("System", "string.Join", "HIGH"),
        ("System.IO", "System.IO.File.ReadAllText", "HIGH"),
        ("System.Text", "StringBuilder", "HIGH"),
        ("System.Text", "StringBuilder.Append", "MEDIUM"),
        ("System.Text", "StringBuilder.Append", "HIGH"),
        ("System.Collections.Generic", "List.Add", "MEDIUM"),
        ("System", "Math.Max", "HIGH"),
        ("System", "int.Parse", "HIGH"),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    // The file's own `Path`, a repository type, and — with `using
    // Acme.Utils` possibly bringing its own `File` — an unqualified `File`.
    assert!(
        calls.contains(&("Report.cs::Path.Combine", "", "")),
        "{calls:?}"
    );
    assert!(calls.contains(&("Total", "", "")), "{calls:?}");
    assert!(calls.contains(&("Join", "", "")), "{calls:?}");
    assert!(calls.contains(&("ReadAllText", "", "")), "{calls:?}");

    let import = |target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "IMPORTS_FROM" && edge.target == target)
            .unwrap_or_else(|| panic!("no import {target}"))
    };
    let text = import("System.Text");
    assert_eq!(text.extra["stdlib"], true);
    assert_eq!(text.extra["confidence_tier"], "HIGH");
    assert_eq!(import("Acme.Utils").extra.get("stdlib"), None);

    // Without a `using` from elsewhere, .NET's implicit global usings make
    // an unqualified `File` likely `System.IO`'s.
    let (_, edges) = parse_csharp(
        "Tool.cs",
        b"class Tool { void Run() { File.ReadAllText(\"x\"); } }\n",
    );
    let read = edges
        .iter()
        .find(|edge| edge.kind == "CALLS")
        .expect("no call");
    assert_eq!(read.target, "System.IO");
    assert_eq!(read.extra["external_symbol"], "File.ReadAllText");
    assert_eq!(read.extra["confidence_tier"], "MEDIUM");
}

#[test]
fn csharp_receivers_record_the_call_they_came_from() {
    let source = br#"using Acme.Data;

class Service
{
    private Repo _repo;
    private Cache Cache { get; } = new Cache();

    async Task<User> Users(Store store)
    {
        store.Open().Fetch();
        var conn = factory.Connect();
        conn.Execute();
        var user = await LoadAsync();
        user.Rename();
        (await GetAsync()).Touch();
        _repo.Save();
        this._repo.Flush();
        new Repo().Load();
        Cache.Get();
        plugin.Run();
        return null;
    }
}

class Cache
{
    public object Get() { return null; }
}
"#;
    let (nodes, edges) = parse_csharp("Service.cs", source);
    let call = |target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target)
            .unwrap_or_else(|| panic!("no {target} in {edges:#?}"))
    };
    let users = nodes
        .iter()
        .find(|node| node.name == "Users")
        .expect("Users");
    assert_eq!(users.return_type.as_deref(), Some("Task<User>"));
    // Declared as a type of another file: parameter, field, `this.` field,
    // constructor.
    assert_eq!(call("Open").extra["receiver_type"], "Store");
    assert_eq!(call("Save").extra["receiver_type"], "Repo");
    assert_eq!(call("Flush").extra["receiver_type"], "Repo");
    assert_eq!(call("Load").extra["receiver_type"], "Repo");
    // A property typed by a class of this file binds its method.
    assert_eq!(
        call("Service.cs::Cache.Get").extra.get("receiver_unknown"),
        None
    );
    // An untyped receiver.
    assert_eq!(call("Run").extra["receiver_unknown"], true);
    assert_eq!(call("Run").extra.get("receiver_from"), None);
    // Receivers that are call results, directly, awaited, or through a
    // variable.
    assert_eq!(
        call("Fetch").extra["receiver_from"],
        serde_json::json!({"call": "Open", "line": 10, "unwrap": false})
    );
    assert_eq!(
        call("Execute").extra["receiver_from"],
        serde_json::json!({"call": "Connect", "line": 11, "unwrap": false})
    );
    assert_eq!(
        call("Rename").extra["receiver_from"],
        serde_json::json!({"call": "LoadAsync", "line": 13, "unwrap": true})
    );
    assert_eq!(
        call("Touch").extra["receiver_from"],
        serde_json::json!({"call": "GetAsync", "line": 15, "unwrap": true})
    );
}
