use crate::{GraphError, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    db_path: String,
    provider: String,
    stamp_ns: u128,
}

#[derive(Debug)]
struct EmbeddingMatrix {
    names: Vec<String>,
    dim: usize,
    rows: Vec<f32>,
}

static EMBEDDING_SEARCH_CACHE: OnceLock<Mutex<HashMap<CacheKey, Arc<EmbeddingMatrix>>>> =
    OnceLock::new();

/// Search provider-partitioned embeddings using a native Rust cosine scan.
///
/// Vectors are stored by Python as native-endian float32 blobs. Rows are loaded
/// into a process-level normalized row-major matrix keyed by `(db_path,
/// provider, db/wal mtime)` so repeated searches avoid SQLite and decode work.
pub fn embedding_search(
    db_path: impl AsRef<Path>,
    provider: &str,
    query_vec: &[f32],
    limit: usize,
) -> Result<Vec<(String, f32)>> {
    if limit == 0 || query_vec.is_empty() || provider.is_empty() {
        return Ok(Vec::new());
    }

    let query_norm = l2_norm(query_vec);
    if query_norm == 0.0 || !query_norm.is_finite() {
        return Ok(Vec::new());
    }
    let query = query_vec
        .iter()
        .map(|value| value / query_norm)
        .collect::<Vec<_>>();

    let matrix = load_embedding_matrix_cached(db_path.as_ref(), provider)?;
    if matrix.names.is_empty() || matrix.dim != query.len() {
        return Ok(Vec::new());
    }

    let scores = matrix_scores(&matrix, &query);
    let top = top_k_scores(&scores, limit);

    Ok(top
        .iter()
        .map(|(row_idx, score)| (matrix.names[*row_idx].clone(), *score))
        .collect())
}

/// Load and cache the native embedding matrix for a provider without running a query.
pub fn embedding_search_prewarm(db_path: impl AsRef<Path>, provider: &str) -> Result<usize> {
    if provider.is_empty() {
        return Ok(0);
    }
    let matrix = load_embedding_matrix_cached(db_path.as_ref(), provider)?;
    Ok(matrix.names.len())
}

fn load_embedding_matrix_cached(db_path: &Path, provider: &str) -> Result<Arc<EmbeddingMatrix>> {
    let stamp_ns = db_stamp_ns(db_path);
    let key = CacheKey {
        db_path: db_path.to_string_lossy().into_owned(),
        provider: provider.to_owned(),
        stamp_ns,
    };
    let cache = EMBEDDING_SEARCH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Some(matrix) = cache
        .lock()
        .map_err(|err| {
            GraphError::InvalidEmbedding(format!("embedding cache lock poisoned: {err}"))
        })?
        .get(&key)
        .cloned()
    {
        return Ok(matrix);
    }

    let matrix = Arc::new(load_embedding_matrix(db_path, provider)?);
    let mut guard = cache.lock().map_err(|err| {
        GraphError::InvalidEmbedding(format!("embedding cache lock poisoned: {err}"))
    })?;
    guard.retain(|existing, _| {
        !(existing.db_path == key.db_path
            && existing.provider == key.provider
            && existing.stamp_ns != key.stamp_ns)
    });
    Ok(guard.entry(key).or_insert_with(|| matrix).clone())
}

fn load_embedding_matrix(db_path: &Path, provider: &str) -> Result<EmbeddingMatrix> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.pragma_update(None, "mmap_size", 268_435_456_i64)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;

    let has_embeddings = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type IN ('table', 'view') AND name = 'embeddings'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !has_embeddings {
        return Ok(EmbeddingMatrix {
            names: Vec::new(),
            dim: 0,
            rows: Vec::new(),
        });
    }

    let (row_count, uniform_dim) = embedding_row_shape_hint(&conn, provider)?;
    let mut stmt =
        conn.prepare("SELECT qualified_name, vector FROM embeddings WHERE provider = ?")?;
    let mut rows = stmt.query(params![provider])?;
    let mut names = Vec::with_capacity(row_count);
    let mut values = uniform_dim
        .and_then(|dim| row_count.checked_mul(dim))
        .map(Vec::with_capacity)
        .unwrap_or_default();
    let mut dim = None;

    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        let blob: Vec<u8> = row.get(1)?;
        let row_dim = blob_dim(&blob)?;
        if row_dim == 0 {
            continue;
        }
        match dim {
            Some(expected) if expected != row_dim => continue,
            None => dim = Some(row_dim),
            _ => {}
        }

        append_normalized_blob(&blob, &mut values)?;
        names.push(name);
    }

    Ok(EmbeddingMatrix {
        names,
        dim: dim.unwrap_or(0),
        rows: values,
    })
}

fn embedding_row_shape_hint(conn: &Connection, provider: &str) -> Result<(usize, Option<usize>)> {
    let (count, min_len, max_len): (i64, Option<i64>, Option<i64>) = conn.query_row(
        "SELECT COUNT(*), MIN(length(vector)), MAX(length(vector)) FROM embeddings WHERE provider = ?",
        params![provider],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let uniform_dim = match (min_len, max_len) {
        (Some(min_len), Some(max_len))
            if min_len == max_len && min_len >= 0 && min_len % 4 == 0 =>
        {
            Some((min_len / 4) as usize)
        }
        _ => None,
    };
    Ok((count.max(0) as usize, uniform_dim))
}

fn blob_dim(blob: &[u8]) -> Result<usize> {
    if !blob.len().is_multiple_of(4) {
        return Err(GraphError::InvalidEmbedding(format!(
            "embedding vector blob length {} is not divisible by 4",
            blob.len()
        )));
    }
    Ok(blob.len() / 4)
}

fn append_normalized_blob(blob: &[u8], out: &mut Vec<f32>) -> Result<()> {
    let row_dim = blob_dim(blob)?;
    let row_start = out.len();
    out.reserve(row_dim);

    let mut norm_sq = 0.0;
    for chunk in blob.chunks_exact(4) {
        let value = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        norm_sq += value * value;
        out.push(value);
    }

    let norm = norm_sq.sqrt();
    if norm == 0.0 || !norm.is_finite() {
        out[row_start..].fill(0.0);
    } else {
        let inv_norm = 1.0 / norm;
        for value in &mut out[row_start..] {
            *value *= inv_norm;
        }
    }
    Ok(())
}

fn matrix_scores(matrix: &EmbeddingMatrix, query: &[f32]) -> Vec<f32> {
    let mut scores = Vec::with_capacity(matrix.names.len());
    for row_idx in 0..matrix.names.len() {
        let start = row_idx * matrix.dim;
        let row = &matrix.rows[start..start + matrix.dim];
        scores.push(dot(row, query));
    }
    scores
}

fn l2_norm(values: &[f32]) -> f32 {
    dot(values, values).sqrt()
}

#[cfg(target_arch = "aarch64")]
fn dot(left: &[f32], right: &[f32]) -> f32 {
    // SAFETY: `dot_neon` only performs in-bounds unaligned loads over slices
    // checked by its loop condition, then handles the scalar remainder.
    unsafe { dot_neon(left, right) }
}

#[cfg(target_arch = "aarch64")]
unsafe fn dot_neon(left: &[f32], right: &[f32]) -> f32 {
    use std::arch::aarch64::{vaddvq_f32, vdupq_n_f32, vfmaq_f32, vld1q_f32};

    let len = left.len().min(right.len());
    let mut i = 0;
    let mut acc0 = vdupq_n_f32(0.0);
    let mut acc1 = vdupq_n_f32(0.0);
    let mut acc2 = vdupq_n_f32(0.0);
    let mut acc3 = vdupq_n_f32(0.0);

    while i + 16 <= len {
        let a0 = vld1q_f32(left.as_ptr().add(i));
        let b0 = vld1q_f32(right.as_ptr().add(i));
        let a1 = vld1q_f32(left.as_ptr().add(i + 4));
        let b1 = vld1q_f32(right.as_ptr().add(i + 4));
        let a2 = vld1q_f32(left.as_ptr().add(i + 8));
        let b2 = vld1q_f32(right.as_ptr().add(i + 8));
        let a3 = vld1q_f32(left.as_ptr().add(i + 12));
        let b3 = vld1q_f32(right.as_ptr().add(i + 12));
        acc0 = vfmaq_f32(acc0, a0, b0);
        acc1 = vfmaq_f32(acc1, a1, b1);
        acc2 = vfmaq_f32(acc2, a2, b2);
        acc3 = vfmaq_f32(acc3, a3, b3);
        i += 16;
    }

    while i + 4 <= len {
        let a = vld1q_f32(left.as_ptr().add(i));
        let b = vld1q_f32(right.as_ptr().add(i));
        acc0 = vfmaq_f32(acc0, a, b);
        i += 4;
    }

    let mut sum = vaddvq_f32(acc0) + vaddvq_f32(acc1) + vaddvq_f32(acc2) + vaddvq_f32(acc3);
    while i < len {
        sum += *left.get_unchecked(i) * *right.get_unchecked(i);
        i += 1;
    }
    sum
}

#[cfg(target_arch = "x86_64")]
fn dot(left: &[f32], right: &[f32]) -> f32 {
    if std::is_x86_feature_detected!("avx") {
        // SAFETY: AVX support is checked at runtime; the implementation only
        // performs in-bounds unaligned loads and scalar remainder handling.
        unsafe { dot_avx(left, right) }
    } else {
        // SAFETY: SSE is guaranteed on x86_64; the implementation only
        // performs in-bounds unaligned loads and scalar remainder handling.
        unsafe { dot_sse(left, right) }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
unsafe fn dot_avx(left: &[f32], right: &[f32]) -> f32 {
    use std::arch::x86_64::{
        _mm256_add_ps, _mm256_loadu_ps, _mm256_mul_ps, _mm256_setzero_ps, _mm256_storeu_ps,
    };

    let len = left.len().min(right.len());
    let mut i = 0;
    let mut acc0 = _mm256_setzero_ps();
    let mut acc1 = _mm256_setzero_ps();
    let mut acc2 = _mm256_setzero_ps();
    let mut acc3 = _mm256_setzero_ps();

    while i + 32 <= len {
        let a0 = _mm256_loadu_ps(left.as_ptr().add(i));
        let b0 = _mm256_loadu_ps(right.as_ptr().add(i));
        let a1 = _mm256_loadu_ps(left.as_ptr().add(i + 8));
        let b1 = _mm256_loadu_ps(right.as_ptr().add(i + 8));
        let a2 = _mm256_loadu_ps(left.as_ptr().add(i + 16));
        let b2 = _mm256_loadu_ps(right.as_ptr().add(i + 16));
        let a3 = _mm256_loadu_ps(left.as_ptr().add(i + 24));
        let b3 = _mm256_loadu_ps(right.as_ptr().add(i + 24));
        acc0 = _mm256_add_ps(acc0, _mm256_mul_ps(a0, b0));
        acc1 = _mm256_add_ps(acc1, _mm256_mul_ps(a1, b1));
        acc2 = _mm256_add_ps(acc2, _mm256_mul_ps(a2, b2));
        acc3 = _mm256_add_ps(acc3, _mm256_mul_ps(a3, b3));
        i += 32;
    }

    while i + 8 <= len {
        let a = _mm256_loadu_ps(left.as_ptr().add(i));
        let b = _mm256_loadu_ps(right.as_ptr().add(i));
        acc0 = _mm256_add_ps(acc0, _mm256_mul_ps(a, b));
        i += 8;
    }

    let acc = _mm256_add_ps(_mm256_add_ps(acc0, acc1), _mm256_add_ps(acc2, acc3));
    let mut lanes = [0.0_f32; 8];
    _mm256_storeu_ps(lanes.as_mut_ptr(), acc);
    let mut sum = lanes.iter().sum::<f32>();
    while i < len {
        sum += *left.get_unchecked(i) * *right.get_unchecked(i);
        i += 1;
    }
    sum
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse")]
unsafe fn dot_sse(left: &[f32], right: &[f32]) -> f32 {
    use std::arch::x86_64::{_mm_add_ps, _mm_loadu_ps, _mm_mul_ps, _mm_setzero_ps, _mm_storeu_ps};

    let len = left.len().min(right.len());
    let mut i = 0;
    let mut acc0 = _mm_setzero_ps();
    let mut acc1 = _mm_setzero_ps();
    let mut acc2 = _mm_setzero_ps();
    let mut acc3 = _mm_setzero_ps();

    while i + 16 <= len {
        let a0 = _mm_loadu_ps(left.as_ptr().add(i));
        let b0 = _mm_loadu_ps(right.as_ptr().add(i));
        let a1 = _mm_loadu_ps(left.as_ptr().add(i + 4));
        let b1 = _mm_loadu_ps(right.as_ptr().add(i + 4));
        let a2 = _mm_loadu_ps(left.as_ptr().add(i + 8));
        let b2 = _mm_loadu_ps(right.as_ptr().add(i + 8));
        let a3 = _mm_loadu_ps(left.as_ptr().add(i + 12));
        let b3 = _mm_loadu_ps(right.as_ptr().add(i + 12));
        acc0 = _mm_add_ps(acc0, _mm_mul_ps(a0, b0));
        acc1 = _mm_add_ps(acc1, _mm_mul_ps(a1, b1));
        acc2 = _mm_add_ps(acc2, _mm_mul_ps(a2, b2));
        acc3 = _mm_add_ps(acc3, _mm_mul_ps(a3, b3));
        i += 16;
    }

    while i + 4 <= len {
        let a = _mm_loadu_ps(left.as_ptr().add(i));
        let b = _mm_loadu_ps(right.as_ptr().add(i));
        acc0 = _mm_add_ps(acc0, _mm_mul_ps(a, b));
        i += 4;
    }

    let acc = _mm_add_ps(_mm_add_ps(acc0, acc1), _mm_add_ps(acc2, acc3));
    let mut lanes = [0.0_f32; 4];
    _mm_storeu_ps(lanes.as_mut_ptr(), acc);
    let mut sum = lanes.iter().sum::<f32>();
    while i < len {
        sum += *left.get_unchecked(i) * *right.get_unchecked(i);
        i += 1;
    }
    sum
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn dot(left: &[f32], right: &[f32]) -> f32 {
    dot_scalar(left, right)
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn dot_scalar(left: &[f32], right: &[f32]) -> f32 {
    let mut sum0 = 0.0;
    let mut sum1 = 0.0;
    let mut sum2 = 0.0;
    let mut sum3 = 0.0;
    let mut left_chunks = left.chunks_exact(4);
    let mut right_chunks = right.chunks_exact(4);
    for (left_chunk, right_chunk) in (&mut left_chunks).zip(&mut right_chunks) {
        sum0 += left_chunk[0] * right_chunk[0];
        sum1 += left_chunk[1] * right_chunk[1];
        sum2 += left_chunk[2] * right_chunk[2];
        sum3 += left_chunk[3] * right_chunk[3];
    }
    let mut sum = sum0 + sum1 + sum2 + sum3;
    for (left_value, right_value) in left_chunks.remainder().iter().zip(right_chunks.remainder()) {
        sum += left_value * right_value;
    }
    sum
}

fn top_k_scores(scores: &[f32], limit: usize) -> Vec<(usize, f32)> {
    let k = limit.min(scores.len());
    if k == 0 {
        return Vec::new();
    }

    if k <= 64 && k.saturating_mul(16) < scores.len() {
        return top_k_scores_small(scores, k);
    }

    let mut indexed = scores.iter().copied().enumerate().collect::<Vec<_>>();
    if k == indexed.len() {
        indexed.sort_unstable_by(|left, right| compare_score_desc(left.1, right.1));
    } else {
        indexed.select_nth_unstable_by(k, |left, right| compare_score_desc(left.1, right.1));
        indexed[..k].sort_unstable_by(|left, right| compare_score_desc(left.1, right.1));
        indexed.truncate(k);
    }
    indexed
}

fn top_k_scores_small(scores: &[f32], k: usize) -> Vec<(usize, f32)> {
    let mut top: Vec<(usize, f32)> = Vec::with_capacity(k);
    let mut min_pos = 0;
    for (idx, score) in scores.iter().copied().enumerate() {
        if top.len() < k {
            top.push((idx, score));
            if top.len() == k {
                min_pos = min_score_position(&top);
            }
            continue;
        }

        if score
            .partial_cmp(&top[min_pos].1)
            .is_some_and(|ordering| ordering == Ordering::Greater)
        {
            top[min_pos] = (idx, score);
            min_pos = min_score_position(&top);
        }
    }
    top.sort_unstable_by(|left, right| compare_score_desc(left.1, right.1));
    top
}

fn min_score_position(values: &[(usize, f32)]) -> usize {
    let mut min_pos = 0;
    for idx in 1..values.len() {
        if values[idx]
            .1
            .partial_cmp(&values[min_pos].1)
            .is_some_and(|ordering| ordering == Ordering::Less)
        {
            min_pos = idx;
        }
    }
    min_pos
}

fn compare_score_desc(left: f32, right: f32) -> Ordering {
    right.partial_cmp(&left).unwrap_or(Ordering::Equal)
}

fn db_stamp_ns(db_path: &Path) -> u128 {
    let mut stamp = file_mtime_ns(db_path);
    for sibling in wal_siblings(db_path) {
        stamp = stamp.max(file_mtime_ns(&sibling));
    }
    stamp
}

fn wal_siblings(db_path: &Path) -> [PathBuf; 2] {
    let raw = db_path.as_os_str().to_string_lossy();
    [
        PathBuf::from(format!("{raw}-wal")),
        PathBuf::from(format!("{raw}-shm")),
    ]
}

fn file_mtime_ns(path: &Path) -> u128 {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("dagayn-{name}-{nonce}.db"))
    }

    fn encode_vector(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect::<Vec<_>>()
    }

    fn seed_embeddings(db_path: &Path, rows: &[(&str, &[f32], &str)]) {
        let conn = Connection::open(db_path).expect("open temp db");
        conn.execute_batch(
            "CREATE TABLE embeddings (
                qualified_name TEXT NOT NULL,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'unknown',
                PRIMARY KEY (qualified_name, provider)
            );",
        )
        .expect("create embeddings table");
        for (name, vector, provider) in rows {
            conn.execute(
                "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) VALUES (?, ?, 'h', ?)",
                params![name, encode_vector(vector), provider],
            )
            .expect("insert embedding row");
        }
    }

    #[test]
    fn native_embedding_search_returns_top_k() {
        let db_path = temp_db_path("top-k");
        seed_embeddings(
            &db_path,
            &[
                ("best", &[1.0, 0.0], "fake"),
                ("other", &[0.0, 1.0], "fake"),
                ("ignored-provider", &[1.0, 0.0], "other"),
            ],
        );

        let results = embedding_search(&db_path, "fake", &[1.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "best");
        assert!((results[0].1 - 1.0).abs() < 1e-6);
        assert_eq!(results[1].0, "other");

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn native_embedding_search_handles_empty_inputs() {
        let db_path = temp_db_path("empty-inputs");
        seed_embeddings(&db_path, &[("best", &[1.0, 0.0], "fake")]);

        assert!(embedding_search(&db_path, "fake", &[1.0, 0.0], 0)
            .unwrap()
            .is_empty());
        assert!(embedding_search(&db_path, "fake", &[0.0, 0.0], 5)
            .unwrap()
            .is_empty());
        assert!(embedding_search(&db_path, "missing", &[1.0, 0.0], 5)
            .unwrap()
            .is_empty());
        assert_eq!(embedding_search_prewarm(&db_path, "fake").unwrap(), 1);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn native_embedding_search_rejects_malformed_blob() {
        let db_path = temp_db_path("bad-blob");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE embeddings (
                qualified_name TEXT NOT NULL,
                vector BLOB NOT NULL,
                text_hash TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'unknown',
                PRIMARY KEY (qualified_name, provider)
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO embeddings (qualified_name, vector, text_hash, provider) VALUES ('bad', x'000102', 'h', 'fake')",
            [],
        )
        .unwrap();
        drop(conn);

        let error = embedding_search(&db_path, "fake", &[1.0], 1).unwrap_err();
        assert!(error.to_string().contains("not divisible by 4"));

        let _ = std::fs::remove_file(db_path);
    }
}
