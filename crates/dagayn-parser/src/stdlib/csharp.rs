//! The .NET base class library (the `System` namespaces).

/// Common types of the base class library by namespace, so a type named
/// without its namespace (`Console`, `List<T>`, `File`) can be traced to
/// it.
const BCL_TYPES: &[(&str, &[&str])] = &[
    (
        "System",
        &[
            "Activator",
            "ArgumentException",
            "ArgumentNullException",
            "ArgumentOutOfRangeException",
            "Array",
            "BitConverter",
            "Boolean",
            "Buffer",
            "Byte",
            "Char",
            "Console",
            "Convert",
            "DateOnly",
            "DateTime",
            "DateTimeOffset",
            "Decimal",
            "Double",
            "Enum",
            "Environment",
            "Exception",
            "GC",
            "Guid",
            "Int16",
            "Int32",
            "Int64",
            "InvalidOperationException",
            "Lazy",
            "Math",
            "MathF",
            "NotImplementedException",
            "NotSupportedException",
            "Nullable",
            "Object",
            "OperatingSystem",
            "Random",
            "Single",
            "String",
            "StringComparer",
            "TimeOnly",
            "TimeSpan",
            "Tuple",
            "Type",
            "UInt32",
            "UInt64",
            "Uri",
            "ValueTuple",
        ],
    ),
    (
        "System.Collections.Generic",
        &[
            "Comparer",
            "Dictionary",
            "EqualityComparer",
            "HashSet",
            "ICollection",
            "IDictionary",
            "IEnumerable",
            "IList",
            "KeyValuePair",
            "LinkedList",
            "List",
            "PriorityQueue",
            "Queue",
            "SortedDictionary",
            "SortedList",
            "SortedSet",
            "Stack",
        ],
    ),
    (
        "System.Collections.Concurrent",
        &[
            "BlockingCollection",
            "ConcurrentBag",
            "ConcurrentDictionary",
            "ConcurrentQueue",
        ],
    ),
    (
        "System.Collections.Immutable",
        &["ImmutableArray", "ImmutableDictionary", "ImmutableList"],
    ),
    ("System.Linq", &["Enumerable", "Queryable"]),
    (
        "System.IO",
        &[
            "BinaryReader",
            "BinaryWriter",
            "Directory",
            "DirectoryInfo",
            "File",
            "FileInfo",
            "FileStream",
            "MemoryStream",
            "Path",
            "Stream",
            "StreamReader",
            "StreamWriter",
            "StringReader",
            "StringWriter",
            "TextReader",
            "TextWriter",
        ],
    ),
    ("System.Text", &["Encoding", "StringBuilder"]),
    ("System.Text.RegularExpressions", &["Match", "Regex"]),
    ("System.Text.Json", &["JsonDocument", "JsonSerializer"]),
    (
        "System.Threading",
        &[
            "CancellationToken",
            "CancellationTokenSource",
            "Interlocked",
            "Monitor",
            "Mutex",
            "SemaphoreSlim",
            "Thread",
            "Timer",
        ],
    ),
    (
        "System.Threading.Tasks",
        &["Parallel", "Task", "TaskCompletionSource", "ValueTask"],
    ),
    (
        "System.Diagnostics",
        &["Debug", "Process", "ProcessStartInfo", "Stopwatch", "Trace"],
    ),
    (
        "System.Net.Http",
        &["HttpClient", "HttpRequestMessage", "HttpResponseMessage"],
    ),
    ("System.Net", &["Dns", "IPAddress", "WebUtility"]),
    ("System.Globalization", &["CultureInfo"]),
    ("System.Reflection", &["Assembly", "BindingFlags"]),
    (
        "System.Runtime.InteropServices",
        &["Marshal", "NativeLibrary", "RuntimeInformation"],
    ),
    (
        "System.Security.Cryptography",
        &["Aes", "RandomNumberGenerator", "SHA256"],
    ),
];

/// C# keywords that name `System` types: `string.Join` is `String.Join`.
const CSHARP_KEYWORD_TYPES: &[&str] = &[
    "bool", "byte", "char", "decimal", "double", "float", "int", "long", "object", "sbyte",
    "short", "string", "uint", "ulong", "ushort",
];

/// The namespace of a common base-class-library type named without it
/// (`StringBuilder` -> `System.Text`).
pub(crate) fn csharp_bcl_namespace(type_name: &str) -> Option<&'static str> {
    BCL_TYPES
        .iter()
        .find(|(_, types)| types.contains(&type_name))
        .map(|(namespace, _)| *namespace)
}

pub(crate) fn is_csharp_keyword_type(name: &str) -> bool {
    CSHARP_KEYWORD_TYPES.contains(&name)
}

/// Whether a namespace belongs to the base class library: `System` or one
/// under it.
pub(crate) fn is_csharp_bcl_namespace(namespace: &str) -> bool {
    namespace == "System" || namespace.starts_with("System.")
}
