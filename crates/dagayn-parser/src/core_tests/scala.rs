use super::*;

#[test]
fn parses_scala_types_calls_imports_and_bridges() {
    let source = br#"import scala.collection.mutable.{HashMap, ListBuffer}
import java.nio.file.Files

trait Repository[T]:
  def save(entity: T): Unit

final case class User(id: Int)

class InMemoryRepo extends Repository[User] with Serializable:
  private val users = mutable.HashMap[Int, User]()

  override def save(user: User): Unit =
    users.put(user.id, user)
    Files.writeString(Path.of("output.json"), "{}")

object BridgeSamples:
  def runCommand(): Unit =
    Runtime.getRuntime().exec("git status")

  def loadLib(): Unit =
    System.loadLibrary("mylib")
"#;
    let (nodes, edges) = parse_scala("sample.scala", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Repository"
            && node.extra["type_role"] == "trait"
            && node.extra["is_contract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.extra["type_role"] == "record"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "save"
            && node.parent_name.as_deref() == Some("InMemoryRepo")
            && node.params.as_deref() == Some("(user: User)")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.target == "scala.collection.mutable"
            && edge.extra["external_symbol"] == "scala.collection.mutable.HashMap"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPLEMENTS"
            && edge.source == "sample.scala::InMemoryRepo"
            && edge.target == "Serializable"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.scala::InMemoryRepo"
            && edge.target == "HashMap"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "Runtime.getRuntime().exec"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:Files.writeString@sample.scala:14>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib"
            && edge.extra["evidence_source"] == "System.loadLibrary"
    }));
}

#[test]
fn scala_standard_library_calls_target_their_package() {
    let source = br#"import scala.collection.mutable.ListBuffer
import scala.collection.mutable
import java.io._
import com.acme.util.Strings

object Report:
  def run(names: List[String], check: () => Unit): Unit =
    println(names)
    val buf = new ListBuffer[Int]()
    buf.append(1)
    mutable.Map.empty[Int, Int]()
    math.max(1, 2)
    scala.math.min(1, 2)
    System.currentTimeMillis()
    new File("x").exists()
    names.map(n => n)
    check()
    require(true)
    Strings.join(names)
    helper.add("c")

  def require(value: Boolean): Unit = ()
"#;
    let (_, edges) = parse_scala("src/Report.scala", source);
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
        ("scala", "println", "MEDIUM"),
        ("scala.collection.mutable", "ListBuffer", "HIGH"),
        ("scala.collection.mutable", "ListBuffer.append", "MEDIUM"),
        ("scala.collection.mutable", "mutable.Map.empty", "HIGH"),
        ("scala.math", "math.max", "MEDIUM"),
        ("scala.math", "scala.math.min", "HIGH"),
        ("java.lang", "System.currentTimeMillis", "HIGH"),
        ("java.io", "File", "HIGH"),
        ("java.io", "File.exists", "HIGH"),
        ("scala", "List.map", "MEDIUM"),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    // A parameter, the file's own `require`, a repository class, and an
    // untyped receiver.
    assert!(calls.contains(&("check", "", "")), "{calls:?}");
    assert!(calls.contains(&("require", "", "")), "{calls:?}");
    assert!(calls.contains(&("join", "", "")), "{calls:?}");
    assert!(calls.contains(&("add", "", "")), "{calls:?}");

    let import = |symbol: &str| {
        edges
            .iter()
            .find(|edge| {
                edge.kind == "IMPORTS_FROM"
                    && edge
                        .extra
                        .get("external_symbol")
                        .map_or(edge.target == symbol, |written| written == symbol)
            })
            .unwrap_or_else(|| panic!("no import {symbol}"))
    };
    let buffer = import("scala.collection.mutable.ListBuffer");
    assert_eq!(buffer.target, "scala.collection.mutable");
    assert_eq!(buffer.extra["confidence_tier"], "HIGH");
    assert_eq!(import("java.io.*").target, "java.io");
    assert_eq!(
        import("scala.collection.mutable").target,
        "scala.collection"
    );
    assert_eq!(import("com.acme.util.Strings").extra.get("stdlib"), None);
}

#[test]
fn scala_receivers_record_the_call_they_came_from() {
    let source = br#"package app

import com.acme.Repo

class Service(repo: Repo) {
  def users(store: Store): Future[User] = {
    store.open().fetch()
    val conn = factory.connect()
    conn.execute()
    repo.save()
    this.repo.flush()
    new Repo().load()
    cache.get()
    Repo.create()
    items.foreach(_.run())
    null
  }

  private val cache = new Cache()
}

class Cache {
  def get(): Any = null
}
"#;
    let (nodes, edges) = parse_scala("src/app/Service.scala", source);
    let call = |target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target)
            .unwrap_or_else(|| panic!("no {target} in {edges:#?}"))
    };
    let users = nodes
        .iter()
        .find(|node| node.name == "users")
        .expect("users");
    assert_eq!(users.return_type.as_deref(), Some("Future[User]"));
    // Declared as a class of another file: parameter, class parameter,
    // `this.` member, constructor.
    assert_eq!(call("open").extra["receiver_type"], "Store");
    assert_eq!(call("save").extra["receiver_type"], "Repo");
    assert_eq!(call("flush").extra["receiver_type"], "Repo");
    assert_eq!(call("load").extra["receiver_type"], "Repo");
    // A class of this file keeps the same-file binding, wherever the member
    // is defined.
    assert_eq!(
        call("src/app/Service.scala::Cache.get")
            .extra
            .get("receiver_unknown"),
        None
    );
    // A call on an object, and untyped receivers.
    assert_eq!(call("create").extra.get("receiver_unknown"), None);
    assert_eq!(call("run").extra["receiver_unknown"], true);
    assert_eq!(call("connect").extra["receiver_unknown"], true);
    // Receivers that are call results, directly or through a variable.
    assert_eq!(
        call("fetch").extra["receiver_from"],
        serde_json::json!({"call": "open", "line": 7, "unwrap": false})
    );
    assert_eq!(
        call("execute").extra["receiver_from"],
        serde_json::json!({"call": "connect", "line": 8, "unwrap": false})
    );
}
