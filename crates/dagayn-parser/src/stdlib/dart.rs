//! The Dart core libraries: `dart:core`, in scope in every library, and the
//! `dart:` libraries a file imports (`dart:io`, `dart:convert`).

/// Names `dart:core` exports, which every library sees without an import
/// (`print(x)`, `DateTime.now()`, `int.parse(s)`, `Future.value(1)`).
const DART_CORE_NAMES: &[&str] = &[
    "ArgumentError",
    "AssertionError",
    "BigInt",
    "Comparable",
    "DateTime",
    "Duration",
    "Enum",
    "Error",
    "Exception",
    "Expando",
    "Finalizer",
    "FormatException",
    "Function",
    "Future",
    "Iterable",
    "Iterator",
    "List",
    "Map",
    "MapEntry",
    "Null",
    "Object",
    "Pattern",
    "RangeError",
    "Record",
    "RegExp",
    "Runes",
    "Set",
    "StateError",
    "Stopwatch",
    "Stream",
    "String",
    "StringBuffer",
    "Symbol",
    "Type",
    "TypeError",
    "UnimplementedError",
    "UnsupportedError",
    "Uri",
    "WeakReference",
    "bool",
    "double",
    "identical",
    "identityHashCode",
    "int",
    "num",
    "print",
];

/// Common top-level names of the other `dart:` libraries, which a file sees
/// once it imports the library without a prefix (`File` after
/// `import 'dart:io'`). `Future` and `Stream` are `dart:core`'s.
const DART_LIBRARY_NAMES: &[(&str, &[&str])] = &[
    (
        "dart:async",
        &[
            "Completer",
            "StreamController",
            "StreamSubscription",
            "Timer",
            "Zone",
            "runZoned",
            "runZonedGuarded",
            "scheduleMicrotask",
            "unawaited",
        ],
    ),
    (
        "dart:collection",
        &[
            "DoubleLinkedQueue",
            "HashMap",
            "HashSet",
            "LinkedHashMap",
            "LinkedHashSet",
            "ListQueue",
            "Queue",
            "SplayTreeMap",
            "SplayTreeSet",
            "UnmodifiableListView",
            "UnmodifiableMapView",
        ],
    ),
    (
        "dart:convert",
        &[
            "JsonDecoder",
            "JsonEncoder",
            "LineSplitter",
            "Utf8Decoder",
            "Utf8Encoder",
            "ascii",
            "base64",
            "base64Decode",
            "base64Encode",
            "base64Url",
            "json",
            "jsonDecode",
            "jsonEncode",
            "latin1",
            "utf8",
        ],
    ),
    ("dart:developer", &["Timeline", "debugger", "log"]),
    (
        "dart:ffi",
        &[
            "DynamicLibrary",
            "NativeCallable",
            "NativeFunction",
            "Pointer",
        ],
    ),
    (
        "dart:io",
        &[
            "Directory",
            "File",
            "HttpClient",
            "HttpServer",
            "InternetAddress",
            "Link",
            "Platform",
            "Process",
            "ServerSocket",
            "Socket",
            "WebSocket",
            "exit",
            "sleep",
            "stderr",
            "stdin",
            "stdout",
        ],
    ),
    ("dart:isolate", &["Isolate", "ReceivePort", "SendPort"]),
    (
        "dart:math",
        &[
            "Point",
            "Random",
            "Rectangle",
            "atan2",
            "cos",
            "exp",
            "log",
            "max",
            "min",
            "pow",
            "sin",
            "sqrt",
            "tan",
        ],
    ),
    (
        "dart:typed_data",
        &[
            "ByteData",
            "Float32List",
            "Float64List",
            "Int16List",
            "Int32List",
            "Int64List",
            "Int8List",
            "Uint16List",
            "Uint32List",
            "Uint8List",
        ],
    ),
];

/// The `dart:` library a URI names (`dart:io`, `dart:convert`), without a
/// subpath; `package:` and relative URIs are not the standard library.
pub(crate) fn dart_library(uri: &str) -> Option<&str> {
    let name = uri.strip_prefix("dart:")?;
    let name = name.split('/').next().filter(|name| !name.is_empty())?;
    Some(&uri[..5 + name.len()])
}

pub(crate) fn is_dart_core_name(name: &str) -> bool {
    DART_CORE_NAMES.contains(&name)
}

/// Whether `library` (`dart:io`) exports the top-level `name` (`File`), by
/// [`DART_LIBRARY_NAMES`].
pub(crate) fn dart_library_exports(library: &str, name: &str) -> bool {
    DART_LIBRARY_NAMES
        .iter()
        .any(|(candidate, names)| *candidate == library && names.contains(&name))
}
