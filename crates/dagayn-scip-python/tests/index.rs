use std::path::{Path, PathBuf};

use dagayn_scip_python::{IndexError, IndexReport, index};
use protobuf::Message;
use scip::types::{Document, Index, Occurrence, PositionEncoding};

const ROLE_DEFINITION: i32 = 1;
const ROLE_IMPORT: i32 = 2;
const ROLE_READ_ACCESS: i32 = 8;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct Shop {
    dir: tempfile::TempDir,
    root: PathBuf,
}

impl Shop {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("shop");
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        copy_dir(&fixtures.join("shop"), &root);
        Self { dir, root }
    }

    /// A virtual environment outside the project; with `pydantic`, the
    /// fixture's stand-in for it is installed. No interpreter: ty reads
    /// `pyvenv.cfg` and the `site-packages` layout.
    fn venv(&self, name: &str, pydantic: bool) -> PathBuf {
        let venv = self.dir.path().join(name);
        let site_packages = venv.join("lib/python3.12/site-packages");
        std::fs::create_dir_all(&site_packages).unwrap();
        std::fs::write(
            venv.join("pyvenv.cfg"),
            format!(
                "home = {}\ninclude-system-site-packages = false\nversion_info = 3.12.4\n",
                self.dir.path().join("no-python/bin").display()
            ),
        )
        .unwrap();
        if pydantic {
            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
            copy_dir(
                &fixtures.join("site-packages/pydantic"),
                &site_packages.join("pydantic"),
            );
        }
        venv
    }

    fn index(&self, python: Option<&Path>) -> Result<(IndexReport, Index), IndexError> {
        let output = self.dir.path().join("index.scip");
        let report = index(&self.root, python, &output)?;
        let bytes = std::fs::read(&output).unwrap();
        Ok((report, Index::parse_from_bytes(&bytes).unwrap()))
    }
}

fn document<'a>(index: &'a Index, path: &str) -> &'a Document {
    index
        .documents
        .iter()
        .find(|d| d.relative_path == path)
        .unwrap_or_else(|| panic!("no document {path}"))
}

/// Occurrences at the `nth` (0-based) whole-word `name` on `line`.
fn occurrences_at<'a>(
    document: &'a Document,
    text: &str,
    line: i32,
    name: &str,
    nth: usize,
) -> Vec<&'a Occurrence> {
    let source_line = text.lines().nth(line as usize).unwrap();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let at = source_line
        .match_indices(name)
        .filter(|(at, _)| {
            !source_line[..*at].chars().next_back().is_some_and(is_word)
                && !source_line[at + name.len()..]
                    .chars()
                    .next()
                    .is_some_and(is_word)
        })
        .nth(nth)
        .unwrap_or_else(|| panic!("no {name} #{nth} on line {line}"))
        .0;
    let column = source_line[..at].encode_utf16().count() as i32;
    document
        .occurrences
        .iter()
        .filter(|o| o.range[0] == line && o.range[1] == column)
        .collect()
}

fn symbols(occurrences: &[&Occurrence]) -> Vec<String> {
    occurrences.iter().map(|o| o.symbol.clone()).collect()
}

const MAIN: &str = include_str!("fixtures/shop/shop/main.py");
const CART: &str = include_str!("fixtures/shop/shop/cart.py");

fn symbols_at(document: &Document, text: &str, line: i32, name: &str) -> Vec<String> {
    symbols(&occurrences_at(document, text, line, name, 0))
}

const SHOP: &str = "scip-python python shop . ";

#[test]
fn symbols_follow_scip_python_grammar_and_ranges_count_utf16_units() {
    let shop = Shop::new();
    let venv = shop.venv("venv", true);
    let (report, index) = shop.index(Some(&venv)).unwrap();
    assert_eq!(report.documents, 3);
    assert_eq!(report.python_version, "3.12");
    assert!(
        report.definitions > 0 && report.references > 0,
        "{report:?}"
    );
    let paths: Vec<&str> = index
        .documents
        .iter()
        .map(|d| d.relative_path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec!["shop/__init__.py", "shop/cart.py", "shop/main.py"]
    );
    for document in &index.documents {
        assert_eq!(
            document.position_encoding.enum_value(),
            Ok(PositionEncoding::UTF16CodeUnitOffsetFromLineStart),
            "{}",
            document.relative_path
        );
    }

    let main = document(&index, "shop/main.py");
    let cart = document(&index, "shop/cart.py");
    assert_eq!(main.occurrences[0].range, vec![0, 0, 0]);
    assert_eq!(
        main.occurrences[0].symbol,
        format!("{SHOP}`shop.main`/__init__:")
    );
    assert_eq!(main.occurrences[0].symbol_roles, ROLE_DEFINITION);

    let total = occurrences_at(main, MAIN, 15, "total", 0);
    assert_eq!(symbols(&total), vec![format!("{SHOP}`shop.cart`/total().")]);
    assert_eq!(total[0].range, vec![15, 34, 39]);
    assert_eq!(total[0].symbol_roles, ROLE_READ_ACCESS);
    assert_eq!(
        symbols_at(cart, CART, 3, "total"),
        vec![format!("{SHOP}`shop.cart`/total().")]
    );
    assert_eq!(
        occurrences_at(cart, CART, 3, "total", 0)[0].symbol_roles,
        ROLE_DEFINITION
    );

    let imported = occurrences_at(main, MAIN, 2, "Cart", 0);
    assert_eq!(symbols(&imported), vec![format!("{SHOP}`shop.cart`/Cart#")]);
    assert_eq!(imported[0].symbol_roles, ROLE_IMPORT);
    assert_eq!(
        symbols_at(main, MAIN, 15, "add"),
        vec![format!("{SHOP}`shop.cart`/Cart#add().")]
    );
    assert_eq!(
        symbols_at(main, MAIN, 15, "prices"),
        vec![format!("{SHOP}`shop.cart`/total().(prices)")]
    );
    assert_eq!(
        symbols_at(main, MAIN, 15, "items"),
        vec![format!("{SHOP}`shop.cart`/Cart#items.")]
    );
    let items = occurrences_at(cart, CART, 12, "items", 0);
    assert_eq!(
        symbols(&items),
        vec![format!("{SHOP}`shop.cart`/Cart#items.")]
    );
    assert_eq!(items[0].symbol_roles, ROLE_DEFINITION);
    assert_eq!(
        symbols_at(main, MAIN, 19, "len"),
        vec!["scip-python python python-stdlib 3.12 builtins/len()."]
    );
}

#[test]
fn nested_functions_and_parameters_are_global_and_body_names_local() {
    let shop = Shop::new();
    let venv = shop.venv("venv", true);
    let (_, index) = shop.index(Some(&venv)).unwrap();
    let cart = document(&index, "shop/cart.py");
    let main = document(&index, "shop/main.py");
    let line = format!("{SHOP}`shop.cart`/Cart#summary().line().");
    assert_eq!(symbols_at(cart, CART, 19, "line"), vec![line.clone()]);
    assert_eq!(symbols_at(cart, CART, 22, "line"), vec![line]);
    assert_eq!(
        symbols_at(cart, CART, 19, "amount"),
        vec![format!("{SHOP}`shop.cart`/Cart#summary().line().(amount)")]
    );
    assert_eq!(symbols_at(cart, CART, 4, "subtotal"), vec!["local 1"]);
    assert_eq!(symbols_at(cart, CART, 7, "subtotal"), vec!["local 1"]);
    assert_eq!(symbols_at(main, MAIN, 15, "label"), vec!["local 1"]);
    assert_eq!(symbols_at(main, MAIN, 18, "label"), vec!["local 1"]);
}

#[test]
fn installed_packages_resolve_in_the_named_environment_only() {
    let shop = Shop::new();
    let model_dump = "scip-python python pydantic . pydantic/BaseModel#model_dump().";

    let venv = shop.venv("venv", true);
    let (report, index) = shop.index(Some(&venv)).unwrap();
    assert_eq!(
        report.environment,
        Some(format!(
            "{} (`--python` argument)",
            venv.canonicalize().unwrap().display()
        ))
    );
    let main = document(&index, "shop/main.py");
    assert_eq!(symbols_at(main, MAIN, 17, "model_dump"), vec![model_dump]);
    assert_eq!(
        symbols_at(main, MAIN, 10, "BaseModel"),
        vec!["scip-python python pydantic . pydantic/BaseModel#"]
    );

    let empty = shop.venv("empty", false);
    let (_, index) = shop.index(Some(&empty)).unwrap();
    let main = document(&index, "shop/main.py");
    assert_eq!(
        symbols_at(main, MAIN, 17, "model_dump"),
        Vec::<String>::new()
    );
    assert_eq!(
        symbols_at(main, MAIN, 15, "total"),
        vec![format!("{SHOP}`shop.cart`/total().")]
    );
}

#[test]
fn a_name_bound_two_ways_gives_one_range_and_two_symbols() {
    let shop = Shop::new();
    let venv = shop.venv("venv", true);
    let (report, index) = shop.index(Some(&venv)).unwrap();
    assert!(report.ambiguous > 0, "{report:?}");
    let main = document(&index, "shop/main.py");
    let codec = occurrences_at(main, MAIN, 18, "codec", 0);
    assert_eq!(
        symbols(&codec),
        vec![
            "scip-python python python-stdlib 3.12 json/__init__:",
            "scip-python python python-stdlib 3.12 pickle/__init__:",
        ]
    );
    assert_eq!(codec[0].range, codec[1].range);
}

#[test]
fn an_environment_inside_the_project_is_not_indexed() {
    let shop = Shop::new();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let env = shop.root.join("env");
    let site_packages = env.join("lib/python3.12/site-packages");
    copy_dir(
        &fixtures.join("site-packages/pydantic"),
        &site_packages.join("pydantic"),
    );
    std::fs::write(
        env.join("pyvenv.cfg"),
        "home = /no-python/bin\nversion_info = 3.12.4\n",
    )
    .unwrap();
    let (report, index) = shop.index(Some(&env)).unwrap();
    assert_eq!(
        report.documents,
        3,
        "{:?}",
        index
            .documents
            .iter()
            .map(|d| &d.relative_path)
            .collect::<Vec<_>>()
    );
}

#[test]
fn an_environment_that_is_not_one_is_a_settings_error() {
    let shop = Shop::new();
    let missing = shop.dir.path().join("no-venv");
    match shop.index(Some(&missing)).unwrap_err() {
        IndexError::Settings(message) => assert!(!message.is_empty()),
        other => panic!("{other:?}"),
    }
    assert!(!shop.dir.path().join("index.scip").exists());
}

#[test]
fn an_unwritable_output_is_a_write_error() {
    let shop = Shop::new();
    let venv = shop.venv("venv", true);
    let output = shop.root.join("missing-dir/index.scip");
    match index(&shop.root, Some(&venv), &output).unwrap_err() {
        IndexError::Write { path, .. } => assert_eq!(path, output),
        other => panic!("{other:?}"),
    }
}
