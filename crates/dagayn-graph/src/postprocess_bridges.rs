use crate::*;

pub(super) fn extra_json(extra: &Value) -> Result<String> {
    Ok(serde_json::to_string(extra)?)
}

#[cfg(test)]
#[path = "postprocess_bridges_tests.rs"]
mod tests;
