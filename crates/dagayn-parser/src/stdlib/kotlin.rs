//! The Kotlin standard library, over the Java class library it runs on.

/// Root packages of the libraries every Kotlin/JVM file can reach: the
/// Kotlin standard library (`kotlin.math`, `kotlin.collections`) and the
/// Java class library (`java.io`).
pub(crate) const KOTLIN_STDLIB_ROOTS: &[&str] = &["kotlin", "java", "javax", "jdk"];

/// Functions of the packages Kotlin imports into every file (`kotlin`,
/// `kotlin.collections`, `kotlin.io`, `kotlin.text`, ...), called bare.
const KOTLIN_BUILTIN_FUNCTIONS: &[&str] = &[
    "TODO",
    "also",
    "apply",
    "arrayListOf",
    "arrayOf",
    "arrayOfNulls",
    "assert",
    "buildList",
    "buildMap",
    "buildSet",
    "buildString",
    "check",
    "checkNotNull",
    "emptyArray",
    "emptyList",
    "emptyMap",
    "emptySequence",
    "emptySet",
    "error",
    "generateSequence",
    "hashMapOf",
    "hashSetOf",
    "intArrayOf",
    "lazy",
    "let",
    "linkedMapOf",
    "linkedSetOf",
    "listOf",
    "listOfNotNull",
    "maxOf",
    "minOf",
    "mutableListOf",
    "mutableMapOf",
    "mutableSetOf",
    "print",
    "println",
    "readLine",
    "readln",
    "repeat",
    "require",
    "requireNotNull",
    "run",
    "runCatching",
    "sequenceOf",
    "setOf",
    "sortedMapOf",
    "sortedSetOf",
    "synchronized",
    "takeIf",
    "takeUnless",
    "to",
    "with",
];

/// Types of the packages Kotlin imports into every file, including the
/// aliases it gives Java classes (`ArrayList`, `IllegalStateException`):
/// named bare, they are Kotlin's even where `java.lang` has one too
/// (`String`).
const KOTLIN_BUILTIN_TYPES: &[&str] = &[
    "Any",
    "Array",
    "ArrayList",
    "Boolean",
    "Byte",
    "Char",
    "CharSequence",
    "Collection",
    "Comparable",
    "Double",
    "Error",
    "Exception",
    "Float",
    "HashMap",
    "HashSet",
    "IllegalArgumentException",
    "IllegalStateException",
    "IndexOutOfBoundsException",
    "Int",
    "Iterable",
    "Iterator",
    "LinkedHashMap",
    "LinkedHashSet",
    "List",
    "Long",
    "Map",
    "MutableList",
    "MutableMap",
    "MutableSet",
    "NoSuchElementException",
    "NullPointerException",
    "Number",
    "NumberFormatException",
    "Pair",
    "Regex",
    "Result",
    "RuntimeException",
    "Sequence",
    "Set",
    "Short",
    "String",
    "StringBuilder",
    "Throwable",
    "Triple",
    "UnsupportedOperationException",
];

pub(crate) fn is_kotlin_builtin_function(name: &str) -> bool {
    KOTLIN_BUILTIN_FUNCTIONS.contains(&name)
}

pub(crate) fn is_kotlin_builtin_type(name: &str) -> bool {
    KOTLIN_BUILTIN_TYPES.contains(&name)
}
