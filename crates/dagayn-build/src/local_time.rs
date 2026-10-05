//! Local wall-clock timestamps in the formats the Python layer stores.

fn local_tm() -> Option<libc::tm> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as libc::time_t)
        .unwrap_or(0);
    // SAFETY: `localtime_r` writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { !libc::localtime_r(&now, &mut tm).is_null() };
    converted.then_some(tm)
}

fn wall_clock(tm: &libc::tm) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// `time.strftime("%Y-%m-%dT%H:%M:%S")`: local time, no zone suffix.
pub(crate) fn local_timestamp() -> String {
    local_tm().map(|tm| wall_clock(&tm)).unwrap_or_default()
}

/// `datetime.now().astimezone().isoformat(timespec="seconds")`: local time
/// with its UTC offset (`+09:00`; seconds only when the offset has them).
pub(crate) fn local_isoformat() -> String {
    let Some(tm) = local_tm() else {
        return String::new();
    };
    // `c_long`: `i64` here, narrower on 32-bit targets.
    #[allow(clippy::unnecessary_cast)]
    let offset = tm.tm_gmtoff as i64;
    let sign = if offset < 0 { '-' } else { '+' };
    let offset = offset.abs();
    let mut zone = format!("{sign}{:02}:{:02}", offset / 3600, offset % 3600 / 60);
    if offset % 60 != 0 {
        zone.push_str(&format!(":{:02}", offset % 60));
    }
    format!("{}{zone}", wall_clock(&tm))
}

#[cfg(test)]
mod tests {
    use super::{local_isoformat, local_timestamp};

    #[test]
    fn timestamp_has_the_python_shape() {
        let stamp = local_timestamp();
        assert_eq!(stamp.len(), 19, "{stamp}");
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "T");
    }

    #[test]
    fn isoformat_carries_the_offset() {
        let stamp = local_isoformat();
        assert_eq!(stamp.len(), 25, "{stamp}");
        assert!(matches!(&stamp[19..20], "+" | "-"), "{stamp}");
        assert_eq!(&stamp[22..23], ":");
    }
}
