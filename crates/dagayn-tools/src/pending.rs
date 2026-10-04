//! The pending refactor store (`dagayn.refactor.pending._pending_refactors`)
//! for one process: a `rename` preview answered in Rust lands here, and the
//! Python `apply_refactor_tool` reads it back through `_core`.
//!
//! Entries are JSON text in insertion order, as Python's dict keeps them.

use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// `REFACTOR_EXPIRY_SECONDS`.
pub const EXPIRY_SECONDS: f64 = 600.0;

static STORE: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn store() -> MutexGuard<'static, Vec<(String, String)>> {
    STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `time.time()`.
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// `_pending_refactors[id] = json` (in place when the id exists).
pub fn set(id: &str, json: String) {
    let mut entries = store();
    match entries.iter_mut().find(|(key, _)| key == id) {
        Some(slot) => slot.1 = json,
        None => entries.push((id.to_string(), json)),
    }
}

pub fn get(id: &str) -> Option<String> {
    store()
        .iter()
        .find(|(key, _)| key == id)
        .map(|(_, v)| v.clone())
}

pub fn remove(id: &str) -> Option<String> {
    let mut entries = store();
    let index = entries.iter().position(|(key, _)| key == id)?;
    Some(entries.remove(index).1)
}

pub fn keys() -> Vec<String> {
    store().iter().map(|(key, _)| key.clone()).collect()
}

pub fn clear() {
    store().clear();
}

/// `_cleanup_expired()`: drop entries older than the expiry.
pub fn cleanup_expired() -> usize {
    let now = now();
    let mut entries = store();
    let before = entries.len();
    entries.retain(|(_, json)| {
        let created = serde_json::from_str::<Value>(json)
            .ok()
            .and_then(|v| v.get("created_at").and_then(Value::as_f64));
        created.is_none_or(|created| now - created <= EXPIRY_SECONDS)
    });
    before - entries.len()
}

#[cfg(test)]
mod tests {
    #[test]
    fn entries_keep_order_and_expire() {
        super::clear();
        super::set("a", r#"{"created_at": 0.0}"#.to_string());
        super::set("b", format!(r#"{{"created_at": {}}}"#, super::now()));
        super::set("a", r#"{"created_at": 1.0}"#.to_string());
        assert_eq!(super::keys(), vec!["a", "b"]);
        assert_eq!(super::cleanup_expired(), 1);
        assert_eq!(super::keys(), vec!["b"]);
        assert!(super::remove("b").is_some());
        assert!(super::get("b").is_none());
    }
}
