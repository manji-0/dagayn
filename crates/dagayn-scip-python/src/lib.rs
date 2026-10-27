//! SCIP index of a Python project from the ty checker
//! (`docs/plans/SCIP-CALL-RESOLUTION.md`).
//!
//! `dagayn build --scip` runs this through `python -P -m
//! dagayn.scip_python` in a child process, one per project (the outermost
//! directory with a `pyproject.toml`, `setup.py`, or `setup.cfg`): a panic
//! aborts only the indexer, and its memory returns on exit. The caller names
//! the project's environment (a virtual environment or an interpreter), so
//! calls into its installed packages resolve. Each name of a project file is
//! resolved with ty's goto-definition, and the index follows scip-python's
//! symbol grammar (`symbols`), so the overlay reads it as it read
//! scip-python's.

mod occurrences;
mod symbols;

use std::path::{Path, PathBuf};
use std::sync::Once;

use protobuf::{EnumOrUnknown, Message};
use ruff_db::files::File;
use ruff_db::system::{OsSystem, SystemPathBuf};
use scip::types::{Document, Index, Metadata, ToolInfo};
use serde::Serialize;
use ty_project::metadata::Options;
use ty_project::metadata::options::EnvironmentOptions;
use ty_project::metadata::value::RelativePathBuf;
use ty_project::{Db as _, ProjectDatabase, ProjectMetadata};

use occurrences::Counts;
use symbols::{BindingNamer, Project};

/// The ruff commit `[workspace.dependencies]` pins; a test checks they agree.
pub const RUFF_REV: &str = "e139fbd6e8ddd5a1653d2f2c15c1123c090a3456";

/// What one `index` call produced.
#[derive(Debug, Default, Serialize)]
pub struct IndexReport {
    pub documents: usize,
    pub definitions: usize,
    pub references: usize,
    /// References emitted once per symbol of their several definitions.
    pub ambiguous: usize,
    /// Names with no definition ty could name.
    pub unresolved: usize,
    /// The `sys.prefix` of the environment ty resolved installed packages
    /// in, and how it was found (`--python argument` when the caller named
    /// it).
    pub environment: Option<String>,
    pub python_version: String,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// ty could not make a project of the directory, or the named
    /// environment is not one.
    #[error("{0}")]
    Settings(String),
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    /// An indexing thread panicked (a release build aborts instead).
    #[error("an indexing thread panicked")]
    Thread,
}

/// Index the Python files of the project at `project` and write the SCIP
/// index to `output`. `python` is the project's environment: a virtual
/// environment directory or an interpreter; `None` leaves the choice to ty
/// (`VIRTUAL_ENV`, a `.venv` in the project, a `python3` on `PATH`, and with
/// none of those the site-packages of the process's own interpreter).
pub fn index(
    project: &Path,
    python: Option<&Path>,
    output: &Path,
) -> Result<IndexReport, IndexError> {
    install_panic_hook();
    let project = std::path::absolute(project).unwrap_or_else(|_| project.to_path_buf());
    let root = SystemPathBuf::from_path_buf(project)
        .map_err(|path| IndexError::Settings(format!("not a UTF-8 path: {}", path.display())))?;
    let system = OsSystem::new(&root);
    let mut metadata = ProjectMetadata::discover(&root, &system)
        .map_err(|err| IndexError::Settings(err.to_string()))?;
    if let Some(python) = python {
        let python = python.to_str().ok_or_else(|| {
            IndexError::Settings(format!("not a UTF-8 path: {}", python.display()))
        })?;
        metadata.set_override_options(Options {
            environment: Some(EnvironmentOptions {
                python: Some(RelativePathBuf::cli(python)),
                ..EnvironmentOptions::default()
            }),
            ..Options::default()
        });
    }
    let db = ProjectDatabase::fallible(metadata, system)
        .map_err(|err| IndexError::Settings(format!("{err:#}")))?;
    let settings = db.project().program_settings(&db);
    let environment = settings
        .python_environment
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map(|env| format!("{} ({})", env.sys_prefix().as_str(), env.origin()));
    let info = Project {
        name: root
            .file_name()
            .map(str::to_string)
            .unwrap_or_else(|| ".".to_string()),
        version: ".".to_string(),
        python_version: settings.python_version.version.to_string(),
        root: root.clone(),
    };
    let prefix = format!("{root}/");
    let mut files: Vec<(File, String)> = db
        .project()
        .files(&db)
        .into_iter()
        .filter_map(|file| {
            let path = file.path(&db).as_system_path()?.to_string();
            let relative = path.strip_prefix(&prefix)?.to_string();
            let python = relative.ends_with(".py") || relative.ends_with(".pyi");
            let installed = relative
                .split('/')
                .any(|part| part == "site-packages" || part == "dist-packages");
            (python && !installed).then_some((file, relative))
        })
        .collect();
    files.sort_by(|a, b| a.1.cmp(&b.1));

    let threads = ruff_db::max_parallelism().get().min(files.len().max(1));
    let mut results: Vec<(String, Document)> = Vec::with_capacity(files.len());
    let mut counts = Counts::default();
    std::thread::scope(|scope| -> Result<(), IndexError> {
        let handles = (0..threads)
            .map(|t| {
                let db = db.clone();
                let mine: Vec<(File, String)> =
                    files.iter().skip(t).step_by(threads).cloned().collect();
                let info = &info;
                std::thread::Builder::new()
                    .name(format!("dagayn-scip-python-{t}"))
                    .stack_size(ruff_db::STACK_SIZE)
                    .spawn_scoped(scope, move || {
                        let mut namer = BindingNamer::new(info);
                        let mut counts = Counts::default();
                        let documents: Vec<(String, Document)> = mine
                            .into_iter()
                            .map(|(file, relative)| {
                                let document = occurrences::document(
                                    &db,
                                    file,
                                    relative.clone(),
                                    &mut namer,
                                    &mut counts,
                                );
                                (relative, document)
                            })
                            .collect();
                        (documents, counts)
                    })
                    .map_err(|_| IndexError::Thread)
            })
            .collect::<Result<Vec<_>, _>>()?;
        for handle in handles {
            let (documents, part) = handle.join().map_err(|_| IndexError::Thread)?;
            results.extend(documents);
            counts.add(&part);
        }
        Ok(())
    })?;
    results.sort_by(|a, b| a.0.cmp(&b.0));

    let report = IndexReport {
        documents: results.len(),
        definitions: counts.definitions,
        references: counts.references,
        ambiguous: counts.ambiguous,
        unresolved: counts.unresolved,
        environment,
        python_version: info.python_version.clone(),
    };
    let mut tool = ToolInfo::new();
    tool.name = "dagayn-scip-python".to_string();
    tool.version = env!("CARGO_PKG_VERSION").to_string();
    let mut metadata = Metadata::new();
    metadata.tool_info = protobuf::MessageField::some(tool);
    metadata.project_root = format!("file://{root}");
    metadata.text_document_encoding = EnumOrUnknown::from_i32(1);
    let mut index = Index::new();
    index.metadata = protobuf::MessageField::some(metadata);
    index.documents = results.into_iter().map(|(_, document)| document).collect();
    let bytes = index
        .write_to_bytes()
        .expect("a SCIP index serializes to protobuf");
    std::fs::write(output, bytes).map_err(|source| IndexError::Write {
        path: output.to_path_buf(),
        source,
    })?;
    Ok(report)
}

fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown location".to_string());
            let message = info.payload_as_str().unwrap_or("non-string panic payload");
            eprintln!(
                "dagayn-scip-python (ty {}) panicked at {location}: {message}",
                &RUFF_REV[..8]
            );
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::RUFF_REV;

    #[test]
    fn the_rev_named_in_panics_is_the_pinned_one() {
        let workspace =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
                .unwrap();
        let pinned: Vec<&str> = workspace
            .lines()
            .filter(|line| line.starts_with("ruff_") || line.starts_with("ty_"))
            .map(|line| {
                line.split("rev = \"")
                    .nth(1)
                    .unwrap()
                    .split('"')
                    .next()
                    .unwrap()
            })
            .collect();
        assert_eq!(pinned.len(), 9);
        assert!(pinned.iter().all(|rev| *rev == RUFF_REV), "{pinned:?}");
    }
}
