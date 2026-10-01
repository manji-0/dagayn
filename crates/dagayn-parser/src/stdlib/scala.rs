//! The Scala standard library, over the Java class library it runs on.

/// Root packages of the libraries every Scala file can reach: the Scala
/// standard library (`scala.collection`) and the Java class library.
pub(crate) const SCALA_STDLIB_ROOTS: &[&str] = &["scala", "java", "javax", "jdk"];

/// Names `scala._` and `Predef._` bring into every file: functions called
/// bare (`println`, `require`) and the types and companions called to build
/// a value (`List(1, 2)`, `Some(x)`, `Option.empty`).
const SCALA_PREDEF_NAMES: &[&str] = &[
    "Array",
    "BigDecimal",
    "BigInt",
    "Either",
    "IndexedSeq",
    "Iterator",
    "LazyList",
    "Left",
    "List",
    "Map",
    "Nil",
    "None",
    "Option",
    "Ordering",
    "Range",
    "Right",
    "Seq",
    "Set",
    "Some",
    "StringBuilder",
    "Vector",
    "assert",
    "assume",
    "identity",
    "implicitly",
    "locally",
    "print",
    "printf",
    "println",
    "require",
    "summon",
];

/// Subpackages of `scala` reachable by their last name, since `scala._` is
/// imported into every file (`math.max`, `collection.mutable.Map`).
const SCALA_SUBPACKAGES: &[&str] = &["collection", "concurrent", "io", "math", "sys", "util"];

pub(crate) fn is_scala_predef_name(name: &str) -> bool {
    SCALA_PREDEF_NAMES.contains(&name)
}

pub(crate) fn is_scala_subpackage(name: &str) -> bool {
    SCALA_SUBPACKAGES.contains(&name)
}
