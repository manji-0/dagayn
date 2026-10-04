//! The embedding arm of `hybrid_search`
//! (`dagayn.search._embedding_search_with_health`) for the one provider a
//! graph can name on its own: a localhost OpenAI-compatible sidecar whose
//! persisted identity pins the dimension
//! (`openai:<model>@http://127.0.0.1:<port>/v1#dim=<n>#text=<mode>`).
//!
//! The query is embedded over plain HTTP and ranked by the same native
//! cosine scan Python calls. Anything else (no auto-resolved provider, a
//! provider the name cannot revive, no vector of that dimension, the
//! fallbacks Python then tries, or a failed request, which Python retries
//! and remembers) is left to Python by returning `None`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dagayn_graph::GraphStore;
use serde_json::{Map, Value, json};

/// `OpenAIEmbeddingProvider`'s default request timeout.
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
/// A sidecar on this machine answers a connect at once; one that does not
/// is Python's to retry.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// `PARTIAL_EMBEDDING_COVERAGE_THRESHOLD`.
const PARTIAL_COVERAGE: f64 = 0.9;
/// `_EMBEDDING_FAILURE_TTL_SECONDS`.
const FAILURE_TTL: Duration = Duration::from_secs(30);

/// Provider keys whose last request failed, as `_emb_failure_cache` keeps
/// them: Python answers (`search_failed_recent`) until the TTL passes.
static FAILURES: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

fn failed_recently(key: &str) -> bool {
    let mut failures = FAILURES.lock().unwrap_or_else(|p| p.into_inner());
    failures.retain(|(_, at)| at.elapsed() < FAILURE_TTL);
    failures.iter().any(|(failed, _)| failed == key)
}

fn record_failure(key: &str) {
    let mut failures = FAILURES.lock().unwrap_or_else(|p| p.into_inner());
    failures.retain(|(failed, _)| failed != key);
    failures.push((key.to_string(), Instant::now()));
}

/// Which provider `_get_cached_emb_store` would use.
pub(crate) enum Request<'a> {
    /// No provider or model: the one the graph's persisted name revives.
    Persisted,
    /// `provider="openai"` (the server default a `--local-embedding` sidecar
    /// sets), from the `CRG_OPENAI_*` environment.
    Openai {
        provider: &'a str,
        model: Option<&'a str>,
    },
}

/// An `OpenAIEmbeddingProvider` on a plain-http localhost endpoint.
struct Provider {
    api_key: String,
    model: String,
    /// `base_url.rstrip("/")`.
    base_url: String,
    host: String,
    port: u16,
    host_key: String,
    timeout: Duration,
    dimension: Option<i64>,
    max_length: Option<i64>,
}

impl Provider {
    fn new(
        api_key: &str,
        base_url: &str,
        model: &str,
        dimension: Option<i64>,
        max_length: Option<i64>,
        timeout: Duration,
    ) -> Option<Self> {
        if !model.is_ascii() || model.trim() != model || model.is_empty() {
            return None;
        }
        let base_url = base_url.trim_end_matches('/');
        let (host, port, host_key) = parse_http_url(base_url)?;
        Some(Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            base_url: base_url.to_string(),
            host,
            port,
            host_key,
            timeout,
            dimension,
            max_length,
        })
    }

    /// `provider.name`.
    fn name(&self) -> String {
        let mut name = format!("openai:{}@{}", self.model, self.host_key);
        if let Some(max_length) = self.max_length {
            name.push_str(&format!("#max_length={max_length}"));
        }
        if let Some(dimension) = self.dimension {
            name.push_str(&format!("#dim={dimension}"));
        }
        name
    }
}

/// An `int(os.environ[name])` setting, `Ok(None)` when unset or empty and
/// `Err` for anything but plain digits.
fn env_number(name: &str) -> Result<Option<i64>, ()> {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(()),
        Ok(raw) if raw.is_empty() => Ok(None),
        Ok(raw) if raw.bytes().all(|b| b.is_ascii_digit()) => raw.parse().map(Some).map_err(|_| ()),
        Ok(_) => Err(()),
    }
}

/// `get_provider("openai", model=model)` for a localhost endpoint.
fn provider_from_env(model: Option<&str>) -> Option<Provider> {
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let api_key = var("CRG_OPENAI_API_KEY")?;
    let base_url = var("CRG_OPENAI_BASE_URL")?;
    let model = model
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .or_else(|| var("CRG_OPENAI_MODEL"))?;
    let dimension = env_number("CRG_OPENAI_DIMENSION").ok()?;
    let max_length = env_number("CRG_OPENAI_MAX_LENGTH").ok()?;
    // Validated as Python parses it; the batch size never reaches one query.
    env_number("CRG_OPENAI_BATCH_SIZE").ok()?;
    let timeout = env_number("CRG_OPENAI_TIMEOUT")
        .ok()?
        .map_or(DEFAULT_TIMEOUT_SECONDS, |seconds| seconds as u64);
    if timeout == 0 {
        return None;
    }
    Provider::new(
        &api_key,
        &base_url,
        &model,
        dimension,
        max_length,
        Duration::from_secs(timeout),
    )
}

/// `_parse_openai_identity_suffixes`, accepting plain digits only.
fn parse_suffixes(tail: &str) -> Option<(&str, Option<i64>, Option<i64>)> {
    let mut tail = tail;
    let mut max_length = None;
    let mut dimension = None;
    let number = |raw: &str| -> Option<i64> {
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        raw.parse().ok()
    };
    loop {
        if let Some((head, raw)) = tail.rsplit_once("#dim=") {
            if head.is_empty() {
                return None;
            }
            dimension = Some(number(raw)?);
            tail = head;
            continue;
        }
        if let Some((head, raw)) = tail.rsplit_once("#max_length=") {
            if head.is_empty() {
                return None;
            }
            max_length = Some(number(raw)?);
            tail = head;
            continue;
        }
        break;
    }
    Some((tail, max_length, dimension))
}

/// `urlparse` of a plain `http://host[:port][/path]` URL, and
/// `_make_host_key`: `(host, port, host key)`.
fn parse_http_url(url: &str) -> Option<(String, u16, String)> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") || rest.contains(['?', '#', '@', '[', ']', '\\', ' ']) {
        return None;
    }
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => {
            if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (host, Some(port.parse::<u16>().ok()?))
        }
        None => (authority, None),
    };
    let host = host.to_ascii_lowercase();
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "0.0.0.0") {
        return None;
    }
    let host_part = match port {
        Some(port) if port != 0 && port != 80 => format!("{host}:{port}"),
        _ => host.clone(),
    };
    let mut path = path.trim_end_matches('/');
    if let Some(stripped) = path.strip_suffix("/embeddings") {
        path = stripped.trim_end_matches('/');
    }
    Some((
        host,
        port.unwrap_or(80),
        format!("http://{host_part}{path}"),
    ))
}

/// `OpenAIEmbeddingProvider.from_persisted_name`; `None` for anything else.
fn provider_from_name(persisted: &str) -> Option<Provider> {
    let base = persisted.split("#text=").next().unwrap_or(persisted);
    if !base.is_ascii() {
        return None;
    }
    let (model, url) = base.strip_prefix("openai:")?.rsplit_once('@')?;
    let (url, max_length, dimension) = parse_suffixes(url)?;
    if url.is_empty() {
        return None;
    }
    let provider = Provider::new(
        "dagayn-local",
        url,
        model,
        dimension,
        max_length,
        Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
    )?;
    dagayn_build::openai_names_match(base, &provider.name()).then_some(provider)
}

/// `_seed_provider_dimension_from_store`: the `#dim=` of the largest stored
/// partition with this identity, for a provider that does not pin one.
fn seed_dimension(store: &GraphStore, provider: &mut Provider, text_mode: &str) -> Option<()> {
    if provider.dimension.is_some() {
        return Some(());
    }
    let wanted = strip_dim(&format!("{}#text={text_mode}", provider.name())).to_ascii_lowercase();
    for persisted in store.embedding_partitions_by_size().ok()? {
        if !persisted.is_ascii() {
            return None;
        }
        if persisted.is_empty() || strip_dim(&persisted).to_ascii_lowercase() != wanted {
            continue;
        }
        if let Some(index) = persisted.find("#dim=") {
            let digits: String = persisted[index + 5..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if !digits.is_empty() {
                provider.dimension = Some(digits.parse().ok()?);
            }
        }
        break;
    }
    Some(())
}

/// `strip_provider_dimension_suffix`.
fn strip_dim(name: &str) -> String {
    let mut out = String::new();
    let mut rest = name;
    while let Some(index) = rest.find("#dim=") {
        out.push_str(&rest[..index]);
        let digits = rest[index + 5..]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(rest.len(), |end| index + 5 + end);
        if digits == index + 5 {
            out.push_str("#dim=");
            rest = &rest[index + 5..];
        } else {
            rest = &rest[digits..];
        }
    }
    out.push_str(rest);
    out
}

/// `_provider_key_for_lookup` for a provider whose dimension is known: the
/// persisted spelling of the first candidate any row carries.
fn lookup_key(store: &GraphStore, provider_key: &str, name: &str) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    for key in [provider_key, name] {
        for variant in [key.to_string(), strip_dim(key)] {
            if !variant.is_empty() && !candidates.contains(&variant) {
                candidates.push(variant);
            }
        }
    }
    for candidate in &candidates {
        if let Some(spelling) = store.embedding_provider_spelling(candidate).ok()? {
            return Some(spelling);
        }
    }
    Some(provider_key.to_string())
}

/// One plain HTTP/1.1 POST to the sidecar: the response body when it
/// answered 200.
fn post_json(provider: &Provider, body: &str) -> Option<String> {
    let path_start = provider.base_url.find("://")? + 3;
    let path = provider.base_url[path_start..]
        .find('/')
        .map_or("", |index| &provider.base_url[path_start + index..]);
    let address = (provider.host.as_str(), provider.port)
        .to_socket_addrs()
        .ok()?
        .next()?;
    let mut stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).ok()?;
    stream.set_read_timeout(Some(provider.timeout)).ok()?;
    stream.set_write_timeout(Some(provider.timeout)).ok()?;
    // A header value cannot carry a line break.
    if provider.api_key.contains(['\r', '\n']) {
        return None;
    }
    let request = format!(
        "POST {path}/embeddings HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\n\
         Authorization: Bearer {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        provider.host,
        provider.port,
        provider.api_key,
        body.len()
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).ok()?;
    if status.split_whitespace().nth(1) != Some("200") {
        return None;
    }
    let mut length = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.parse::<usize>().ok()?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.eq_ignore_ascii_case("chunked");
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).ok()?;
            let size = usize::from_str_radix(size.trim().split(';').next()?, 16).ok()?;
            if size == 0 {
                break;
            }
            let mut chunk = vec![0; size + 2];
            reader.read_exact(&mut chunk).ok()?;
            chunk.truncate(size);
            body.extend(chunk);
        }
    } else if let Some(length) = length {
        body.resize(length, 0);
        reader.read_exact(&mut body).ok()?;
    } else {
        reader.read_to_end(&mut body).ok()?;
    }
    String::from_utf8(body).ok()
}

/// `embed_query`: `_call_api([query])[0]`, as the `float32` the native scan
/// receives; `None` for any response Python would reject or retry.
fn embed_query(provider: &Provider, query: &str) -> Option<Vec<f32>> {
    let mut body = Map::new();
    body.insert("model".into(), json!(provider.model));
    body.insert("input".into(), json!([query]));
    if let Some(dimension) = provider.dimension {
        body.insert("dimensions".into(), json!(dimension));
    }
    if let Some(max_length) = provider.max_length {
        body.insert("max_length".into(), json!(max_length));
    }
    let raw = post_json(provider, &Value::Object(body).to_string())?;
    let response: Value = serde_json::from_str(&raw).ok()?;
    if response.get("error").is_some() {
        return None;
    }
    let data = response.get("data")?.as_array()?;
    // One input: an unindexed single item, or one indexed 0.
    let [item] = data.as_slice() else {
        return None;
    };
    match item.get("index") {
        None => {}
        Some(index) if index.as_i64() == Some(0) && !index.is_f64() => {}
        Some(_) => return None,
    }
    item.get("embedding")?
        .as_array()?
        .iter()
        .map(|value| value.as_f64().map(|v| v as f32))
        .collect()
}

/// `_embedding_search_with_health(store, query, limit, text_mode=mode)`: the
/// `(node id, score)` hits and the health record, or `None` for Python.
pub(crate) fn search(
    store: &GraphStore,
    db_path: &Path,
    query: &str,
    limit: i64,
    text_mode: &str,
    counts: &HashMap<String, i64>,
    request: &Request,
) -> Option<(Vec<(i64, f64)>, Value)> {
    if std::env::var("DAGAYN_EMBEDDING_SEARCH_BACKEND")
        .is_ok_and(|backend| backend.trim().eq_ignore_ascii_case("python"))
    {
        return None;
    }
    let (hint, mut provider, requested_provider, requested_model) = match request {
        Request::Persisted => {
            let preferred = store.get_metadata("embedding_provider").ok()?;
            let hint = dagayn_build::resolve_active_embedding_provider(
                counts,
                Some(text_mode),
                preferred.as_deref(),
            )?;
            let provider = provider_from_name(&hint)?;
            (Some(hint), provider, Value::Null, Value::Null)
        }
        Request::Openai { provider, model } => (
            None,
            provider_from_env(*model)?,
            json!(provider),
            json!(model),
        ),
    };
    seed_dimension(store, &mut provider, text_mode)?;
    // A provider no stored key pins falls to Python's dimension fallbacks.
    let dimension = provider.dimension?;
    let name = provider.name();
    let provider_key = format!("{name}#text={text_mode}");
    let key = lookup_key(store, &provider_key, &name)?;
    let matching = store
        .count_embeddings(&key, Some(dimension.checked_mul(4)?))
        .ok()?;
    // Python's fallbacks (another hint, the stored dimension, another text
    // mode) and its failure statuses start here.
    if matching == 0 || failed_recently(&provider_key) {
        return None;
    }

    let Some(vector) = embed_query(&provider, query) else {
        record_failure(&provider_key);
        return None;
    };
    let hits = dagayn_graph::embedding_search(db_path, &key, &vector, usize::try_from(limit).ok()?)
        .ok()?;
    let names: Vec<String> = hits.iter().map(|(name, _)| name.clone()).collect();
    let nodes = store.get_nodes_by_qualified_names(&names).ok()?;
    let results: Vec<(i64, f64)> = hits
        .iter()
        .filter_map(|(name, score)| nodes.get(name).map(|node| (node.id, f64::from(*score))))
        .collect();

    let mut health = Map::new();
    health.insert("status".into(), json!("available"));
    health.insert("requested_provider".into(), requested_provider);
    health.insert("requested_model".into(), requested_model);
    health.insert("requested_text_mode".into(), json!(text_mode));
    health.insert("resolved_provider".into(), json!(name));
    health.insert("resolved_provider_key".into(), json!(provider_key));
    let auto = hint
        .as_ref()
        .filter(|hint| **hint == name || **hint == provider_key);
    health.insert("auto_resolved_provider".into(), json!(auto));
    health.insert("matching_vector_count".into(), json!(matching));
    health.insert("provider_counts".into(), json!(counts));
    health.insert("query_dimension".into(), json!(dimension));
    health.insert("resolved_text_mode".into(), json!(text_mode));
    // `_attach_embedding_coverage`.
    let embeddable = store.count_non_file_nodes().ok()?;
    if embeddable > 0 {
        health.insert("embeddable_node_count".into(), json!(embeddable));
        health.insert(
            "missing_embedding_count".into(),
            json!((embeddable - matching).max(0)),
        );
        let coverage = (matching as f64 / embeddable as f64).min(1.0);
        health.insert(
            "embedding_coverage".into(),
            json!(crate::answerability::round4(coverage)),
        );
        if coverage < PARTIAL_COVERAGE {
            health.insert("partial_coverage".into(), json!(true));
            health.insert("status".into(), json!("degraded"));
        }
    }
    Some((results, Value::Object(health)))
}

#[cfg(test)]
mod tests {
    use super::{parse_http_url, provider_from_name, strip_dim};

    #[test]
    fn a_pinned_localhost_identity_revives() {
        let provider = provider_from_name(
            "openai:bge-m3-gguf-q8_0@http://127.0.0.1:18080/v1#dim=1024#text=material",
        )
        .expect("provider");
        assert_eq!(
            provider.name(),
            "openai:bge-m3-gguf-q8_0@http://127.0.0.1:18080/v1#dim=1024"
        );
        assert_eq!(provider.base_url, "http://127.0.0.1:18080/v1");
        assert_eq!(
            (provider.host.as_str(), provider.port),
            ("127.0.0.1", 18080)
        );
        assert_eq!(
            provider_from_name("openai:m@http://127.0.0.1:18080/v1")
                .expect("unpinned")
                .dimension,
            None
        );
        assert!(provider_from_name("openai:m@https://api.openai.com/v1#dim=8").is_none());
        assert!(provider_from_name("google:m#dim=8").is_none());
    }

    #[test]
    fn host_keys_drop_default_ports_and_the_embeddings_path() {
        assert_eq!(
            parse_http_url("http://LOCALHOST:80/v1/embeddings/")
                .expect("url")
                .2,
            "http://localhost/v1"
        );
        assert!(parse_http_url("http://127.0.0.1:99999/v1").is_none());
        assert_eq!(strip_dim("a#dim=12#text=m"), "a#text=m");
    }
}
