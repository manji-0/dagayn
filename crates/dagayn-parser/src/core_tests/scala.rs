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
        edge.kind == "IMPORTS_FROM" && edge.target == "scala.collection.mutable.HashMap"
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
