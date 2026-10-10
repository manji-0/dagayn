//! Python slicing the analysis tools share.

/// `items[:limit]`.
pub(crate) fn py_prefix<T: Clone>(items: &[T], limit: i64) -> Vec<T> {
    let len = items.len() as i64;
    let end = if limit < 0 {
        (len + limit).max(0)
    } else {
        limit.min(len)
    };
    items[..end as usize].to_vec()
}
