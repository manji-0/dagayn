//! The Java class library, shared by the JVM languages (Java, Kotlin, Scala).

/// Root packages of the Java class library: `java.util`, `javax.crypto`,
/// `jdk.jfr`.
pub(crate) const JAVA_STDLIB_ROOTS: &[&str] = &["java", "javax", "jdk"];

/// Common classes of the Java class library by package, so a class named
/// without its package (`Math.max`, `new ArrayList<>()` after
/// `import java.util.*`) can be traced to it. `java.lang` comes first: its
/// classes are in scope in every Java, Kotlin, and Scala file.
const JVM_CLASSES: &[(&str, &[&str])] = &[
    (
        "java.lang",
        &[
            "AutoCloseable",
            "Boolean",
            "Byte",
            "CharSequence",
            "Character",
            "Class",
            "ClassLoader",
            "Double",
            "Enum",
            "Float",
            "Integer",
            "Iterable",
            "Long",
            "Math",
            "Number",
            "Object",
            "Process",
            "ProcessBuilder",
            "Runnable",
            "Runtime",
            "Short",
            "StackWalker",
            "StrictMath",
            "String",
            "StringBuffer",
            "StringBuilder",
            "System",
            "Thread",
            "ThreadLocal",
            "Throwable",
            "Void",
            "Exception",
            "Error",
            "RuntimeException",
            "ArithmeticException",
            "ArrayIndexOutOfBoundsException",
            "ClassCastException",
            "CloneNotSupportedException",
            "IllegalArgumentException",
            "IllegalStateException",
            "IndexOutOfBoundsException",
            "InterruptedException",
            "NullPointerException",
            "NumberFormatException",
            "UnsupportedOperationException",
        ],
    ),
    (
        "java.util",
        &[
            "AbstractMap",
            "ArrayDeque",
            "ArrayList",
            "Arrays",
            "Base64",
            "BitSet",
            "Calendar",
            "Collection",
            "Collections",
            "Comparator",
            "Date",
            "Deque",
            "EnumMap",
            "EnumSet",
            "HashMap",
            "HashSet",
            "Iterator",
            "LinkedHashMap",
            "LinkedHashSet",
            "LinkedList",
            "List",
            "Locale",
            "Map",
            "NoSuchElementException",
            "Objects",
            "Optional",
            "OptionalDouble",
            "OptionalInt",
            "OptionalLong",
            "PriorityQueue",
            "Properties",
            "Queue",
            "Random",
            "Scanner",
            "Set",
            "SortedMap",
            "SortedSet",
            "Stack",
            "StringJoiner",
            "Timer",
            "TreeMap",
            "TreeSet",
            "UUID",
            "Vector",
        ],
    ),
    (
        "java.util.concurrent",
        &[
            "Callable",
            "CompletableFuture",
            "ConcurrentHashMap",
            "ConcurrentLinkedQueue",
            "CopyOnWriteArrayList",
            "CountDownLatch",
            "ExecutorService",
            "Executors",
            "Future",
            "LinkedBlockingQueue",
            "ScheduledExecutorService",
            "Semaphore",
            "ThreadLocalRandom",
            "TimeUnit",
        ],
    ),
    (
        "java.util.concurrent.atomic",
        &[
            "AtomicBoolean",
            "AtomicInteger",
            "AtomicLong",
            "AtomicReference",
        ],
    ),
    (
        "java.util.concurrent.locks",
        &["ReentrantLock", "ReadWriteLock"],
    ),
    (
        "java.util.function",
        &[
            "BiConsumer",
            "BiFunction",
            "BinaryOperator",
            "Consumer",
            "Function",
            "Predicate",
            "Supplier",
            "UnaryOperator",
        ],
    ),
    (
        "java.util.stream",
        &[
            "Collectors",
            "DoubleStream",
            "IntStream",
            "LongStream",
            "Stream",
            "StreamSupport",
        ],
    ),
    ("java.util.regex", &["Matcher", "Pattern"]),
    ("java.util.logging", &["Level", "Logger"]),
    (
        "java.io",
        &[
            "BufferedInputStream",
            "BufferedOutputStream",
            "BufferedReader",
            "BufferedWriter",
            "ByteArrayInputStream",
            "ByteArrayOutputStream",
            "Closeable",
            "File",
            "FileInputStream",
            "FileNotFoundException",
            "FileOutputStream",
            "FileReader",
            "FileWriter",
            "IOException",
            "InputStream",
            "InputStreamReader",
            "ObjectInputStream",
            "ObjectOutputStream",
            "OutputStream",
            "OutputStreamWriter",
            "PrintStream",
            "PrintWriter",
            "Reader",
            "Serializable",
            "StringReader",
            "StringWriter",
            "UncheckedIOException",
            "Writer",
        ],
    ),
    ("java.nio", &["ByteBuffer", "CharBuffer"]),
    ("java.nio.charset", &["Charset", "StandardCharsets"]),
    (
        "java.nio.file",
        &[
            "Files",
            "FileSystems",
            "Path",
            "Paths",
            "StandardCopyOption",
            "StandardOpenOption",
        ],
    ),
    (
        "java.net",
        &[
            "HttpURLConnection",
            "InetAddress",
            "InetSocketAddress",
            "ServerSocket",
            "Socket",
            "URI",
            "URL",
            "URLDecoder",
            "URLEncoder",
        ],
    ),
    (
        "java.net.http",
        &["HttpClient", "HttpRequest", "HttpResponse"],
    ),
    ("java.math", &["BigDecimal", "BigInteger", "RoundingMode"]),
    (
        "java.time",
        &[
            "Clock",
            "Duration",
            "Instant",
            "LocalDate",
            "LocalDateTime",
            "LocalTime",
            "OffsetDateTime",
            "Period",
            "ZoneId",
            "ZoneOffset",
            "ZonedDateTime",
        ],
    ),
    ("java.time.format", &["DateTimeFormatter"]),
    ("java.time.temporal", &["ChronoUnit"]),
    (
        "java.text",
        &["DecimalFormat", "MessageFormat", "SimpleDateFormat"],
    ),
    (
        "java.sql",
        &[
            "Connection",
            "DriverManager",
            "PreparedStatement",
            "ResultSet",
            "SQLException",
            "Statement",
            "Timestamp",
        ],
    ),
    ("java.security", &["MessageDigest", "SecureRandom"]),
];

/// Whether a dotted path starts at one of `roots` (`java.util.List` under
/// [`JAVA_STDLIB_ROOTS`]).
pub(crate) fn jvm_path_has_root(path: &str, roots: &[&str]) -> bool {
    path.split('.')
        .next()
        .is_some_and(|first| roots.contains(&first))
}

/// The package a JVM path names: the segments before its first class (an
/// upper-case segment) — `java.util` for `java.util.Map.Entry` and for
/// `java.lang.Math.max` (a static import). A path without a class names
/// a package member (`kotlin.math.max` -> `kotlin.math`), or, imported
/// with a wildcard, the package itself (`java.io.*` -> `java.io`).
pub(crate) fn jvm_package_of(path: &str, wildcard: bool) -> Option<String> {
    let segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    let class = segments
        .iter()
        .position(|segment| segment.starts_with(|c: char| c.is_ascii_uppercase()));
    let end = match class {
        Some(index) => index,
        None if wildcard => segments.len(),
        None => segments.len().saturating_sub(1),
    };
    (end > 0).then(|| segments[..end].join("."))
}

/// The package of a common class of the Java class library named without
/// its package (`ArrayList` -> `java.util`).
pub(crate) fn jvm_class_package(name: &str) -> Option<&'static str> {
    JVM_CLASSES
        .iter()
        .find(|(_, classes)| classes.contains(&name))
        .map(|(package, _)| *package)
}

/// Whether `name` is a class of `java.lang`, in scope in every JVM file
/// without an import.
pub(crate) fn is_java_lang_class(name: &str) -> bool {
    jvm_class_package(name) == Some("java.lang")
}
