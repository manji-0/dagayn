#[cfg(target_os = "macos")]
fn main() {
    use std::path::PathBuf;
    use std::process::Command;

    if let Ok(output) = Command::new("xcrun").arg("--show-sdk-path").output()
        && output.status.success()
    {
        let sdk_root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !sdk_root.is_empty() {
            println!("cargo:rustc-link-search=native={}/usr/lib", sdk_root);
            return;
        }
    }

    let fallback = PathBuf::from("/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/usr/lib");
    if fallback.exists() {
        println!("cargo:rustc-link-search=native={}", fallback.display());
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
