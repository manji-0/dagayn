//! The Swift standard library (module `Swift`, imported implicitly) and the
//! Apple system frameworks a file imports (`Foundation`, `UIKit`,
//! `SwiftUI`).

/// Modules that ship with the toolchain or the Apple SDKs: an `import` of
/// one (`import Foundation`, `import struct os.OSLog`) names the platform,
/// not a package of the repository or its dependencies.
const SWIFT_SYSTEM_MODULES: &[&str] = &[
    "ARKit",
    "AVFoundation",
    "AVKit",
    "Accelerate",
    "AppKit",
    "AuthenticationServices",
    "CloudKit",
    "Combine",
    "Contacts",
    "CoreAudio",
    "CoreBluetooth",
    "CoreData",
    "CoreFoundation",
    "CoreGraphics",
    "CoreImage",
    "CoreLocation",
    "CoreML",
    "CoreMotion",
    "CoreText",
    "CoreVideo",
    "CryptoKit",
    "Darwin",
    "Dispatch",
    "Distributed",
    "EventKit",
    "Foundation",
    "GameKit",
    "Glibc",
    "HealthKit",
    "LocalAuthentication",
    "MapKit",
    "MessageUI",
    "Metal",
    "MetalKit",
    "NaturalLanguage",
    "Network",
    "OSLog",
    "ObjectiveC",
    "Observation",
    "Photos",
    "PhotosUI",
    "QuartzCore",
    "RealityKit",
    "RegexBuilder",
    "SafariServices",
    "SceneKit",
    "Security",
    "SpriteKit",
    "StoreKit",
    "Swift",
    "SwiftUI",
    "Synchronization",
    "Testing",
    "UIKit",
    "UniformTypeIdentifiers",
    "UserNotifications",
    "Vision",
    "WebKit",
    "WidgetKit",
    "XCTest",
    "os",
];

/// Names of the Swift standard library every file sees: free functions
/// (`print(x)`, `max(a, b)`) and types called as initializers or through
/// their static members (`String(n)`, `Int("3")`, `Array(repeating:)`).
const SWIFT_STDLIB_NAMES: &[&str] = &[
    "AnyHashable",
    "Array",
    "Bool",
    "Character",
    "ClosedRange",
    "ContiguousArray",
    "Dictionary",
    "Double",
    "Float",
    "Int",
    "Int16",
    "Int32",
    "Int64",
    "Int8",
    "KeyValuePairs",
    "MemoryLayout",
    "ObjectIdentifier",
    "Optional",
    "Range",
    "Result",
    "Set",
    "String",
    "Substring",
    "Task",
    "UInt",
    "UInt16",
    "UInt32",
    "UInt64",
    "UInt8",
    "Unmanaged",
    "UnsafeBufferPointer",
    "UnsafeMutablePointer",
    "UnsafeMutableRawPointer",
    "UnsafePointer",
    "UnsafeRawPointer",
    "abs",
    "assert",
    "assertionFailure",
    "debugPrint",
    "dump",
    "fatalError",
    "max",
    "min",
    "numericCast",
    "precondition",
    "preconditionFailure",
    "print",
    "readLine",
    "repeatElement",
    "sequence",
    "stride",
    "swap",
    "type",
    "withCheckedContinuation",
    "withCheckedThrowingContinuation",
    "withTaskGroup",
    "withThrowingTaskGroup",
    "withUnsafeMutablePointer",
    "withUnsafePointer",
    "zip",
];

/// Common names of the system frameworks, which a file sees once it imports
/// the framework or one that re-exports it (`UIKit`, `AppKit`, and
/// `SwiftUI` bring `Foundation`, which brings `Dispatch`).
const SWIFT_FRAMEWORK_NAMES: &[(&str, &[&str])] = &[
    (
        "Foundation",
        &[
            "Bundle",
            "Calendar",
            "CharacterSet",
            "Data",
            "Date",
            "DateComponents",
            "DateFormatter",
            "Decimal",
            "FileHandle",
            "FileManager",
            "ISO8601DateFormatter",
            "IndexPath",
            "JSONDecoder",
            "JSONEncoder",
            "JSONSerialization",
            "Locale",
            "NSError",
            "NSLocalizedString",
            "NSLock",
            "NSNumber",
            "NSObject",
            "NSPredicate",
            "NSRange",
            "NSRegularExpression",
            "NSString",
            "NotificationCenter",
            "NumberFormatter",
            "OperationQueue",
            "Pipe",
            "Process",
            "ProcessInfo",
            "PropertyListDecoder",
            "PropertyListEncoder",
            "RunLoop",
            "Thread",
            "TimeZone",
            "Timer",
            "URL",
            "URLComponents",
            "URLRequest",
            "URLSession",
            "UUID",
            "UserDefaults",
        ],
    ),
    (
        "Dispatch",
        &[
            "DispatchGroup",
            "DispatchQueue",
            "DispatchSemaphore",
            "DispatchTime",
            "DispatchWorkItem",
        ],
    ),
    (
        "UIKit",
        &[
            "UIAlertAction",
            "UIAlertController",
            "UIApplication",
            "UIButton",
            "UIColor",
            "UIDevice",
            "UIFont",
            "UIImage",
            "UIImageView",
            "UILabel",
            "UINavigationController",
            "UIScreen",
            "UIStackView",
            "UITableView",
            "UIView",
            "UIViewController",
            "UIWindow",
        ],
    ),
    (
        "AppKit",
        &[
            "NSAlert",
            "NSApplication",
            "NSButton",
            "NSColor",
            "NSFont",
            "NSImage",
            "NSMenu",
            "NSMenuItem",
            "NSTextField",
            "NSView",
            "NSViewController",
            "NSWindow",
            "NSWorkspace",
        ],
    ),
    (
        "SwiftUI",
        &[
            "AnyView",
            "Button",
            "Color",
            "Divider",
            "ForEach",
            "Form",
            "GeometryReader",
            "Group",
            "HStack",
            "Image",
            "LazyHStack",
            "LazyVStack",
            "NavigationLink",
            "NavigationStack",
            "NavigationView",
            "ProgressView",
            "ScrollView",
            "Section",
            "Spacer",
            "TabView",
            "Text",
            "TextField",
            "Toggle",
            "VStack",
            "ZStack",
        ],
    ),
    (
        "Combine",
        &[
            "AnyCancellable",
            "AnyPublisher",
            "CurrentValueSubject",
            "Just",
            "PassthroughSubject",
        ],
    ),
    ("os", &["Logger", "OSLog", "os_log"]),
    (
        "XCTest",
        &[
            "XCTAssert",
            "XCTAssertEqual",
            "XCTAssertFalse",
            "XCTAssertGreaterThan",
            "XCTAssertLessThan",
            "XCTAssertNil",
            "XCTAssertNoThrow",
            "XCTAssertNotEqual",
            "XCTAssertNotNil",
            "XCTAssertThrowsError",
            "XCTAssertTrue",
            "XCTFail",
            "XCTSkip",
            "XCTUnwrap",
            "XCTestExpectation",
        ],
    ),
];

/// Frameworks an import of the key framework also brings into scope.
const SWIFT_REEXPORTS: &[(&str, &[&str])] = &[
    ("Foundation", &["Dispatch"]),
    ("UIKit", &["Foundation", "Dispatch"]),
    ("AppKit", &["Foundation", "Dispatch"]),
    ("SwiftUI", &["Foundation", "Dispatch"]),
    ("CoreData", &["Foundation", "Dispatch"]),
    ("OSLog", &["os"]),
];

/// The system module an import path names (`Foundation`, `os` for
/// `os.OSLog`), or `None` for a module of the repository or a dependency.
pub(crate) fn swift_system_module(path: &str) -> Option<&'static str> {
    let first = path.split('.').next()?;
    SWIFT_SYSTEM_MODULES
        .iter()
        .find(|module| **module == first)
        .copied()
}

pub(crate) fn is_swift_stdlib_name(name: &str) -> bool {
    SWIFT_STDLIB_NAMES.contains(&name)
}

/// The imported framework (`imported` holds the file's system modules) that
/// provides `name`: `Date` with `Foundation` (or `UIKit`) imported.
pub(crate) fn swift_framework_of(name: &str, imported: &[&str]) -> Option<&'static str> {
    SWIFT_FRAMEWORK_NAMES
        .iter()
        .find(|(framework, names)| {
            names.contains(&name)
                && imported.iter().any(|module| {
                    module == framework
                        || SWIFT_REEXPORTS.iter().any(|(importer, brought)| {
                            importer == module && brought.contains(framework)
                        })
                })
        })
        .map(|(framework, _)| *framework)
}
