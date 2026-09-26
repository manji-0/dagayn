use super::*;

#[test]
fn parses_kotlin_types_calls_imports_and_bridges() {
    let source = br#"import java.nio.file.Files

interface UserRepository {
    fun save(user: User)
}

data class User(val id: Int)

class InMemoryRepo : UserRepository {
    fun save(user: User) {
        println(user)
        Files.writeString(java.nio.file.Path.of("output.txt"), "ok")
    }

    fun run(path: String) {
        Runtime.getRuntime().exec("git status")
        Files.readString(java.nio.file.Path.of(path))
        System.loadLibrary("mylib")
    }
}

fun createUser(repo: UserRepository) {
    val user = User(1)
    repo.save(user)
}
"#;
    let (nodes, edges) = parse_kotlin("sample.kt", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "UserRepository"
            && node.extra["type_role"] == "interface"
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
            && node.params.is_none()
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "sample.kt"
            && edge.target == "java.nio.file.Files"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPLEMENTS"
            && edge.source == "sample.kt::InMemoryRepo"
            && edge.target == "UserRepository"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.kt::createUser"
            && edge.target == "sample.kt::UserRepository.save"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "Runtime.getRuntime().exec"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:Files.readString@sample.kt:17>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib"
            && edge.extra["evidence_source"] == "System.loadLibrary"
    }));
}
