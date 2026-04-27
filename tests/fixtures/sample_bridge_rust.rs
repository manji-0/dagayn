// Fixture for bridge detection tests (Rust).
use std::fs;
use std::process::Command;

fn run_command() {
    let _child = Command::new("git");
}

fn read_config() -> String {
    fs::read_to_string("config.yaml").unwrap()
}

fn write_output(data: &[u8]) {
    fs::write("output.json", data).unwrap();
}

fn read_dynamic(path: &str) -> Vec<u8> {
    // Dynamic path — LOW confidence edge
    fs::read(path).unwrap()
}
