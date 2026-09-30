//! JNI symbol names for `native` (Java) and `external` (Kotlin) methods.
//!
//! The JVM resolves such a method to the C symbol
//! `Java_<mangled binary class name>_<mangled method name>` in any loaded
//! library, so the name alone ties the declaration to its implementation.

/// `Java_com_example_Outer_00024Inner_fastSum` for method `fastSum` of
/// class `Outer.Inner` in package `com.example`.
pub(super) fn jni_symbol(package: Option<&str>, class_path: &str, method: &str) -> String {
    let mut binary = String::new();
    if let Some(package) = package.filter(|package| !package.is_empty()) {
        binary.push_str(&package.replace('.', "/"));
        binary.push('/');
    }
    binary.push_str(&class_path.replace('.', "$"));
    format!("Java_{}_{}", jni_mangle(&binary), jni_mangle(method))
}

/// The JNI name-mangling scheme (JNI specification, "Resolving Native
/// Method Names").
fn jni_mangle(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        match ch {
            '/' => out.push('_'),
            '_' => out.push_str("_1"),
            ';' => out.push_str("_2"),
            '[' => out.push_str("_3"),
            ch if ch.is_ascii_alphanumeric() => out.push(ch),
            ch => {
                let mut units = [0u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    out.push_str(&format!("_0{unit:04x}"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mangles_packages_nested_classes_and_underscores() {
        assert_eq!(
            jni_symbol(Some("com.example"), "Sum", "fastSum"),
            "Java_com_example_Sum_fastSum"
        );
        assert_eq!(
            jni_symbol(Some("org.my_app"), "Outer.Inner", "do_it"),
            "Java_org_my_1app_Outer_00024Inner_do_1it"
        );
        assert_eq!(jni_symbol(None, "Top", "run"), "Java_Top_run");
    }
}
