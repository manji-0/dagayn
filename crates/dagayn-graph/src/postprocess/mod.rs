//! Post-processing passes over the stored graph: bare-name resolution,
//! endpoint demotion, manifest and native-binding bridges, Terraform
//! references, and TESTED_BY synchronisation. Each is an `impl GraphStore`
//! block the Python pipeline (and dagayn-postproc) calls after parsing.

mod bare_names;
mod endpoints;
mod manifest;
mod native_bindings;
mod terraform;
mod tested_by;

pub(crate) use tested_by::sync_tested_by_with_calls;

#[cfg(test)]
mod tests;
