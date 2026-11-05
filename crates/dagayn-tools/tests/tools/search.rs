use dagayn_build::db_path_for_build;
use serde_json::json;

use crate::common::{Repo, answer, declines};

#[test]
fn search_without_embeddings_ranks_fts_hits() {
    // dagayn: tests crates/dagayn-tools/src/search.rs::semantic_search
    let repo = Repo::new("search", false);
    repo.build();
    let context = repo.context();
    let found = answer(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "helper"}),
    );
    assert_eq!(found["search_mode"], "fts_only");
    assert!(found.get("embedding_health").is_none());
    let verbose = answer(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "helper", "detail_level": "verbose"}),
    );
    assert_eq!(
        verbose["embedding_health"]["status"],
        "provider_unavailable"
    );
    assert_eq!(found["results"][0]["qualified_name"], "app.py::helper");
    assert_eq!(found["exactness"]["exact_match_count"], 1);
    let none = answer(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "zz_nothing", "detail_level": "minimal"}),
    );
    assert_eq!(none["result_count"], 0);
    assert_eq!(none["zero_result_reason"], "not_found_in_current_graph");

    // Anything that embeds the query is Python's.
    assert!(declines(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "x", "provider": "openai"})
    ));
    let mut configured = repo.context();
    configured.embedding_provider = Some("openai".to_string());
    assert!(declines(
        &configured,
        "semantic_search_nodes_tool",
        json!({"query": "helper"})
    ));
    let conn = rusqlite::Connection::open(repo.0.join(".dagayn/graph.db")).expect("open");
    conn.execute(
        "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) \
         VALUES ('app.py::main', x'00000000', 'h', 'local#dim=1')",
        [],
    )
    .expect("insert vector");
    assert!(declines(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "helper"})
    ));
}

/// A one-request-at-a-time OpenAI-compatible endpoint that embeds every
/// query as `vector`.
fn fake_embedding_server(vector: [f32; 4]) -> u16 {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let reply = json!({"data": [{"index": 0, "embedding": vector}]}).to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    port
}

#[test]
fn search_ranks_stored_vectors_against_the_sidecar_query() {
    // dagayn: tests crates/dagayn-tools/src/search.rs::semantic_search
    let repo = Repo::new("vectors", true);
    repo.build();
    let port = fake_embedding_server([0.0, 1.0, 0.0, 0.0]);
    let provider = format!("openai:fake-model@http://127.0.0.1:{port}/v1#dim=4#text=material");
    {
        let conn =
            rusqlite::Connection::open(db_path_for_build(&repo.0).expect("db")).expect("open");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS embeddings (qualified_name TEXT NOT NULL, vector BLOB NOT NULL, \
             text_hash TEXT NOT NULL, provider TEXT NOT NULL, PRIMARY KEY (qualified_name, provider))",
        )
        .expect("schema");
        // Two of the three embeddable nodes: partial coverage.
        let rows: [(&str, [f32; 4]); 2] = [
            ("app.py::main", [1.0, 0.0, 0.0, 0.0]),
            ("app.py::helper", [0.0, 1.0, 0.0, 0.0]),
        ];
        for (name, vector) in rows {
            let blob: Vec<u8> = vector.iter().flat_map(|v| v.to_ne_bytes()).collect();
            conn.execute(
                "INSERT INTO embeddings VALUES (?1, ?2, 'h', ?3)",
                rusqlite::params![name, blob, provider],
            )
            .expect("insert");
        }
    }
    let context = repo.context();
    let found = answer(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "something that assists", "limit": 3, "detail_level": "verbose"}),
    );
    let health = &found["embedding_health"];
    assert_eq!(health["status"], "degraded", "{health}");
    assert_eq!(health["resolved_provider_key"], provider);
    assert_eq!(health["auto_resolved_provider"], provider);
    assert_eq!(health["matching_vector_count"], 2);
    assert_eq!(found["search_mode"], "embedding_only", "{found}");
    assert_eq!(found["results"][0]["qualified_name"], "app.py::helper");
    assert_eq!(found["results"][0]["source"], "embedding");
    let codes: Vec<&str> = found["missingness"]
        .as_array()
        .expect("missingness")
        .iter()
        .filter_map(|item| item["reason_code"].as_str())
        .collect();
    assert!(codes.contains(&"partial_embeddings"), "{codes:?}");
    assert!(!codes.contains(&"missing_embeddings"), "{codes:?}");

    // A provider named in the call is Python's.
    assert!(declines(
        &context,
        "semantic_search_nodes_tool",
        json!({"query": "x", "provider": "openai"})
    ));
}
