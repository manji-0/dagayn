//! SCIP index of a TypeScript project from the tsrs checker
//! (`docs/plans/SCIP-CALL-RESOLUTION.md`).
//!
//! `dagayn build --scip` runs this through `python -P -m
//! dagayn.scip_typescript` in a child process, one per project (a directory
//! with a `tsconfig.json`): tsrs is a young port whose panic aborts the
//! process, and its memory returns on exit. The index follows
//! scip-typescript's symbol grammar (`symbols`), so the overlay reads it as
//! it read scip-typescript's, and emits every declaration of an identifier
//! with several (`occurrences`).

mod occurrences;
mod symbols;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};

use protobuf::{EnumOrUnknown, Message};
use scip::types::{Document, Index, Metadata, ToolInfo};
use serde::Serialize;
use tsrs_compiler::{ProgramOptions, new_compiler_host, new_program};
use tsrs_core::P;
use tsrs_core::context::Context;
use tsrs_tsoptions::{self as tsoptions, ParseConfigHost};
use tsrs_vfs::{FS, bundled, osvfs};

use occurrences::Counts;
use symbols::SymbolNamer;

/// The tsrs commit `[workspace.dependencies]` pins; a test checks they agree.
pub const TSRS_REV: &str = "e906c9e4";

/// The checker needs a deep stack; the thread that runs it gets 512 MB.
const STACK_BYTES: usize = 512 << 20;

/// What one `index` call produced.
#[derive(Debug, Default, Serialize)]
pub struct IndexReport {
    /// tsconfigs that produced a program.
    pub configs: usize,
    /// `"<tsconfig>: <first diagnostic>"` for each tsconfig that did not.
    pub skipped_configs: Vec<String>,
    pub documents: usize,
    pub definitions: usize,
    pub references: usize,
    /// Identifiers with no symbol or no declaration.
    pub unresolved: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// Every tsconfig failed to parse; the reasons, one per tsconfig.
    #[error("no tsconfig produced a program: {}", .0.join("; "))]
    NoProgram(Vec<String>),
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The indexing thread panicked (a release build aborts instead).
    #[error("the indexing thread panicked")]
    Thread,
}

struct ConfigHost {
    fs: Arc<dyn FS>,
    cwd: String,
}

impl ParseConfigHost for ConfigHost {
    fn fs(&self) -> &dyn FS {
        &*self.fs
    }
    fn get_current_directory(&self) -> &str {
        &self.cwd
    }
}

/// Index the TypeScript projects `tsconfigs` (relative to `project`) and
/// write the SCIP index to `output`. A tsconfig that gives no program (it
/// cannot be read; TypeScript's parser recovers from malformed JSON) is
/// skipped and reported in `skipped_configs`; only all of them failing is
/// an error. A file several tsconfigs cover is indexed by the first.
pub fn index(
    project: &Path,
    tsconfigs: &[PathBuf],
    output: &Path,
) -> Result<IndexReport, IndexError> {
    install_panic_hook();
    let project = project.to_path_buf();
    let tsconfigs = tsconfigs.to_vec();
    let output = output.to_path_buf();
    std::thread::Builder::new()
        .name("dagayn-scip-typescript".to_string())
        .stack_size(STACK_BYTES)
        .spawn(move || index_on_this_thread(&project, &tsconfigs, &output))
        .map_err(|_| IndexError::Thread)?
        .join()
        .map_err(|_| IndexError::Thread)?
}

/// The three-line stderr tail the build's warning quotes names the tsrs
/// build the panic came from.
fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown location".to_string());
            let message = info.payload_as_str().unwrap_or("non-string panic payload");
            eprintln!("dagayn-scip-typescript (tsrs {TSRS_REV}) panicked at {location}: {message}");
        }));
    });
}

fn index_on_this_thread(
    project: &Path,
    tsconfigs: &[PathBuf],
    output: &Path,
) -> Result<IndexReport, IndexError> {
    let project = std::path::absolute(project).unwrap_or_else(|_| project.to_path_buf());
    let cwd = tsrs_core::tspath::normalize_path(&project.to_string_lossy());
    let fs: Arc<dyn FS> = Arc::new(bundled::wrap_fs(osvfs::fs()));
    let lib_dir = bundled::default_lib_path();
    let mut namer = SymbolNamer::new(lib_dir.clone(), tsrs_core::version().to_string());
    let ctx = Context::background();
    let mut report = IndexReport::default();
    let mut counts = Counts::default();
    let mut documents: Vec<Document> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let prefix = format!("{cwd}/");
    for tsconfig in tsconfigs {
        let tsconfig = tsconfig.to_string_lossy();
        let host = ConfigHost {
            fs: Arc::clone(&fs),
            cwd: cwd.clone(),
        };
        let path = format!("{cwd}/{tsconfig}");
        let (config, diagnostics) =
            tsoptions::get_parsed_command_line_of_config_file(&path, None, None, &host, None);
        let Some(config) = config else {
            let reason = diagnostics
                .first()
                .map(|d| d.message_text().to_string())
                .unwrap_or_else(|| "no program".to_string());
            report.skipped_configs.push(format!("{tsconfig}: {reason}"));
            continue;
        };
        report.configs += 1;
        let roots: HashSet<String> = config.file_names().iter().cloned().collect();
        let host = new_compiler_host(&cwd, Arc::clone(&fs), &lib_dir, None, None, None);
        let program = new_program(ProgramOptions::new(P::new(config), host));
        program.bind_source_files();
        for &sf in program.get_source_files() {
            let name = sf.file_name().to_string();
            if name.contains("/node_modules/")
                || name.starts_with(&lib_dir)
                || !roots.contains(&name)
            {
                continue;
            }
            let relative = match name.strip_prefix(&prefix) {
                Some(r) => r.to_string(),
                None => relative_to(&cwd, &name),
            };
            if !seen.insert(relative.clone()) {
                continue;
            }
            documents.push(occurrences::document(
                program,
                &ctx,
                sf,
                relative,
                &mut namer,
                &mut counts,
            ));
        }
    }
    if report.configs == 0 {
        return Err(IndexError::NoProgram(std::mem::take(
            &mut report.skipped_configs,
        )));
    }
    report.documents = documents.len();
    report.definitions = counts.definitions;
    report.references = counts.references;
    report.unresolved = counts.unresolved;

    let mut tool = ToolInfo::new();
    tool.name = "dagayn-scip-typescript".to_string();
    tool.version = env!("CARGO_PKG_VERSION").to_string();
    let mut metadata = Metadata::new();
    metadata.tool_info = protobuf::MessageField::some(tool);
    metadata.project_root = format!("file://{cwd}");
    metadata.text_document_encoding = EnumOrUnknown::from_i32(1);
    let mut index = Index::new();
    index.metadata = protobuf::MessageField::some(metadata);
    index.documents = documents;
    let bytes = index
        .write_to_bytes()
        .expect("a SCIP index serializes to protobuf");
    std::fs::write(output, bytes).map_err(|source| IndexError::Write {
        path: output.to_path_buf(),
        source,
    })?;
    Ok(report)
}

/// `path` relative to `base`, both absolute with `/` separators.
fn relative_to(base: &str, path: &str) -> String {
    let b: Vec<&str> = base.split('/').filter(|x| !x.is_empty()).collect();
    let p: Vec<&str> = path.split('/').filter(|x| !x.is_empty()).collect();
    let common = b.iter().zip(&p).take_while(|(x, y)| x == y).count();
    let mut out: Vec<&str> = vec![".."; b.len() - common];
    out.extend(&p[common..]);
    out.join("/")
}

#[cfg(test)]
mod tests {
    use super::{TSRS_REV, relative_to};

    #[test]
    fn paths_outside_the_project_climb_to_it() {
        assert_eq!(
            relative_to("/repo/web", "/repo/shared/a.ts"),
            "../shared/a.ts"
        );
        assert_eq!(relative_to("/repo", "/repo/src/a.ts"), "src/a.ts");
    }

    #[test]
    fn the_rev_named_in_panics_is_the_pinned_one() {
        let workspace =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
                .unwrap();
        let pinned: Vec<&str> = workspace
            .lines()
            .filter(|line| line.starts_with("tsrs_"))
            .map(|line| {
                line.split("rev = \"")
                    .nth(1)
                    .unwrap()
                    .split('"')
                    .next()
                    .unwrap()
            })
            .collect();
        assert_eq!(pinned.len(), 6);
        assert!(pinned.iter().all(|rev| *rev == TSRS_REV), "{pinned:?}");
    }
}
