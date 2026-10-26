//! The index of `fixtures/shop`, read back as the overlay reads it.
//!
//! `node_modules` is gitignored, so the fixture keeps its fake package in
//! `vendor_modules/` and each test copies the fixture to a tempdir, laying
//! the package out as pnpm does: `node_modules/left-pad` is a symlink into
//! `node_modules/.pnpm/`, the realpath scip-typescript named `.` (#219).

use std::path::{Path, PathBuf};

use dagayn_scip_typescript::{IndexError, index};
use protobuf::Message;
use scip::types::{Document, Index, Occurrence, PositionEncoding};

const ROLE_DEFINITION: i32 = 1;
const ROLE_IMPORT: i32 = 2;

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
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Shop {
    fn new(with_node_modules: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("shop");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shop");
        copy_dir(&fixture, &root);
        let vendored = root.join("vendor_modules");
        if with_node_modules {
            let store = root.join("node_modules/.pnpm/left-pad@1.3.0/node_modules");
            std::fs::create_dir_all(&store).unwrap();
            std::fs::rename(vendored.join("left-pad"), store.join("left-pad")).unwrap();
            std::os::unix::fs::symlink(store.join("left-pad"), root.join("node_modules/left-pad"))
                .unwrap();
        }
        std::fs::remove_dir_all(&vendored).unwrap();
        Self { _dir: dir, root }
    }

    fn index(
        &self,
        tsconfigs: &[&str],
    ) -> Result<(dagayn_scip_typescript::IndexReport, Index), IndexError> {
        let output = self.root.join("index.scip");
        let configs: Vec<PathBuf> = tsconfigs.iter().map(PathBuf::from).collect();
        let report = index(&self.root, &configs, &output)?;
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

fn occurrences_at<'a>(
    document: &'a Document,
    text: &str,
    line: i32,
    name: &str,
) -> Vec<&'a Occurrence> {
    let source_line = text.lines().nth(line as usize).unwrap();
    let column = source_line[..source_line.find(name).unwrap()]
        .encode_utf16()
        .count() as i32;
    document
        .occurrences
        .iter()
        .filter(|o| o.range[0] == line && o.range[1] == column)
        .collect()
}

fn symbols(occurrences: &[&Occurrence]) -> Vec<String> {
    occurrences.iter().map(|o| o.symbol.clone()).collect()
}

const MAIN: &str = include_str!("fixtures/shop/src/main.ts");
const CART: &str = include_str!("fixtures/shop/src/cart.ts");

#[test]
fn symbols_follow_scip_typescript_grammar_and_ranges_count_utf16_units() {
    let shop = Shop::new(true);
    let (report, index) = shop.index(&["tsconfig.json"]).unwrap();
    assert_eq!(report.configs, 1);
    assert_eq!(report.skipped_configs, Vec::<String>::new());
    assert_eq!(report.documents, 2);
    assert!(
        report.definitions > 0 && report.references > 0,
        "{report:?}"
    );
    for document in &index.documents {
        assert_eq!(
            document.position_encoding.enum_value(),
            Ok(PositionEncoding::UTF16CodeUnitOffsetFromLineStart),
            "{}",
            document.relative_path
        );
    }

    let main = document(&index, "src/main.ts");
    let cart = document(&index, "src/cart.ts");
    assert_eq!(main.occurrences[0].range, vec![0, 0, 0]);
    assert_eq!(
        main.occurrences[0].symbol,
        "scip-typescript npm shop 1.2.3 src/`main.ts`/"
    );

    // `const label = "🛒"; total([1, 2]);`: the cart is two UTF-16 units.
    let total = occurrences_at(main, MAIN, 5, "total");
    assert_eq!(
        symbols(&total),
        vec!["scip-typescript npm shop 1.2.3 src/`cart.ts`/total()."]
    );
    assert_eq!(total[0].range, vec![5, 20, 25]);
    assert_eq!(total[0].symbol_roles, 0);

    let add = occurrences_at(main, MAIN, 4, "add");
    assert_eq!(
        symbols(&add),
        vec!["scip-typescript npm shop 1.2.3 src/`cart.ts`/Cart#add()."]
    );

    let left_pad = "scip-typescript npm left-pad 1.3.0 `index.d.ts`/leftPad().";
    let imported = occurrences_at(main, MAIN, 0, "leftPad");
    assert_eq!(symbols(&imported), vec![left_pad]);
    assert_eq!(imported[0].symbol_roles, ROLE_IMPORT);
    let called = occurrences_at(main, MAIN, 6, "leftPad");
    assert_eq!(symbols(&called), vec![left_pad]);
    assert_eq!(called[0].symbol_roles, 0);

    let max = occurrences_at(cart, CART, 23, "max");
    assert_eq!(max.len(), 1, "{:?}", symbols(&max));
    assert!(
        max[0].symbol.starts_with("scip-typescript npm typescript "),
        "{}",
        max[0].symbol
    );
    assert!(
        max[0].symbol.ends_with(" lib/`lib.es5.d.ts`/Math#max()."),
        "{}",
        max[0].symbol
    );

    let definition = occurrences_at(cart, CART, 0, "total");
    assert_eq!(
        symbols(&definition),
        vec!["scip-typescript npm shop 1.2.3 src/`cart.ts`/total()."]
    );
    assert_eq!(definition[0].symbol_roles, ROLE_DEFINITION);

    // Three declarations of `format` (two overloads and the body) share one symbol.
    let format = occurrences_at(main, MAIN, 6, "format");
    assert_eq!(
        symbols(&format),
        vec!["scip-typescript npm shop 1.2.3 src/`cart.ts`/format()."]
    );
}

#[test]
fn a_merged_declaration_gives_one_range_and_two_symbols() {
    let shop = Shop::new(true);
    let (_, index) = shop.index(&["tsconfig.json"]).unwrap();
    let main = document(&index, "src/main.ts");
    let discount = occurrences_at(main, MAIN, 6, "discount");
    assert_eq!(
        symbols(&discount),
        vec![
            "scip-typescript npm shop 1.2.3 src/`cart.ts`/discount().",
            "scip-typescript npm shop 1.2.3 src/`cart.ts`/discount/",
        ]
    );
    assert_eq!(discount[0].range, discount[1].range);
}

#[test]
fn locals_are_numbered_per_document() {
    let shop = Shop::new(true);
    let (_, index) = shop.index(&["tsconfig.json"]).unwrap();
    let cart = document(&index, "src/cart.ts");
    let main = document(&index, "src/main.ts");
    // The first local of each document is `local 1`: `prices` in cart.ts,
    // the arrow parameter `item` in main.ts.
    assert_eq!(
        symbols(&occurrences_at(cart, CART, 0, "prices")),
        vec!["local 1"]
    );
    assert_eq!(
        symbols(&occurrences_at(main, MAIN, 7, "item")),
        vec!["local 1"]
    );
    // A function declared inside a body is local too, with the id of its
    // declaration wherever it is referenced.
    let label_definition = symbols(&occurrences_at(cart, CART, 31, "label"));
    assert!(
        label_definition[0].starts_with("local "),
        "{label_definition:?}"
    );
    assert_eq!(
        symbols(&occurrences_at(cart, CART, 34, "label")),
        label_definition
    );
}

#[test]
fn an_unreadable_tsconfig_beside_a_good_one_is_skipped() {
    let shop = Shop::new(true);
    std::fs::create_dir(shop.root.join("tsconfig.dir.json")).unwrap();
    let (report, index) = shop.index(&["tsconfig.json", "tsconfig.dir.json"]).unwrap();
    assert_eq!(report.configs, 1);
    assert_eq!(report.skipped_configs.len(), 1, "{report:?}");
    assert!(
        report.skipped_configs[0].starts_with("tsconfig.dir.json: "),
        "{report:?}"
    );
    assert_eq!(index.documents.len(), 2);
}

#[test]
fn a_malformed_tsconfig_is_still_a_program_over_the_files_it_defaults_to() {
    // TypeScript's config parser recovers from bad JSON, so the config is a
    // program whose `include` defaults to every file under the project.
    let shop = Shop::new(true);
    std::fs::write(shop.root.join("tsconfig.broken.json"), "{ not json").unwrap();
    let (report, index) = shop.index(&["tsconfig.broken.json"]).unwrap();
    assert_eq!(report.configs, 1);
    assert_eq!(report.skipped_configs, Vec::<String>::new());
    assert_eq!(index.documents.len(), 2);
}

#[test]
fn only_broken_tsconfigs_are_an_error_and_write_nothing() {
    let shop = Shop::new(true);
    let error = shop.index(&["tsconfig.missing.json"]).unwrap_err();
    match error {
        IndexError::NoProgram(reasons) => {
            assert_eq!(reasons.len(), 1);
            assert!(
                reasons[0].starts_with("tsconfig.missing.json: "),
                "{reasons:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(!shop.root.join("index.scip").exists());
}

#[test]
fn a_file_two_tsconfigs_cover_is_indexed_once_by_the_first() {
    let shop = Shop::new(true);
    std::fs::write(
        shop.root.join("tsconfig.main.json"),
        r#"{"compilerOptions": {"strict": true, "types": []}, "files": ["src/main.ts"]}"#,
    )
    .unwrap();
    let (report, index) = shop
        .index(&["tsconfig.main.json", "tsconfig.json"])
        .unwrap();
    assert_eq!(report.configs, 2);
    let paths: Vec<&str> = index
        .documents
        .iter()
        .map(|d| d.relative_path.as_str())
        .collect();
    assert_eq!(paths, vec!["src/main.ts", "src/cart.ts"]);
}

#[test]
fn without_node_modules_the_package_reference_is_absent_and_the_rest_resolves() {
    let shop = Shop::new(false);
    let (report, index) = shop.index(&["tsconfig.json"]).unwrap();
    assert_eq!(report.documents, 2);
    let main = document(&index, "src/main.ts");
    assert!(
        !main
            .occurrences
            .iter()
            .any(|o| o.symbol.contains("left-pad")),
        "{:?}",
        symbols(&main.occurrences.iter().collect::<Vec<_>>())
    );
    assert_eq!(
        occurrences_at(main, MAIN, 6, "leftPad"),
        Vec::<&Occurrence>::new()
    );
    assert_eq!(
        symbols(&occurrences_at(main, MAIN, 5, "total")),
        vec!["scip-typescript npm shop 1.2.3 src/`cart.ts`/total()."]
    );
}

#[test]
fn an_unwritable_output_is_a_write_error() {
    let shop = Shop::new(true);
    let output = shop.root.join("missing-dir/index.scip");
    let error = index(&shop.root, &[PathBuf::from("tsconfig.json")], &output).unwrap_err();
    match error {
        IndexError::Write { path, .. } => assert_eq!(path, output),
        other => panic!("{other:?}"),
    }
}
