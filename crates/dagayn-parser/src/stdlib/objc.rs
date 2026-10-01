//! Apple's system frameworks for Objective-C (`Foundation`, `UIKit`, ...).
//! The package is the framework, the first component of the header path
//! (`<Foundation/Foundation.h>`) or the module of an `@import`. C functions
//! keep their `libc` / `posix` package; see [`super::c`].

/// SDK frameworks and system modules, as spelled in `#import <X/...>` or
/// `@import X;`.
const OBJC_FRAMEWORKS: &[&str] = &[
    "AVFoundation",
    "Accelerate",
    "AppKit",
    "AudioToolbox",
    "Cocoa",
    "Combine",
    "CoreData",
    "CoreFoundation",
    "CoreGraphics",
    "CoreImage",
    "CoreLocation",
    "CoreMedia",
    "CoreServices",
    "CoreText",
    "CoreVideo",
    "Dispatch",
    "Foundation",
    "GameKit",
    "ImageIO",
    "IOKit",
    "MapKit",
    "Metal",
    "MetalKit",
    "Network",
    "Photos",
    "QuartzCore",
    "SceneKit",
    "Security",
    "SpriteKit",
    "StoreKit",
    "SystemConfiguration",
    "UIKit",
    "UniformTypeIdentifiers",
    "UserNotifications",
    "WebKit",
    "XCTest",
    "dispatch",
    "objc",
    "os",
];

/// The framework an include path or `@import` module names
/// (`Foundation/Foundation.h` -> `Foundation`, `dispatch/dispatch.h` ->
/// `Dispatch`). A bare header (`<stdio.h>`) is not a framework.
pub(crate) fn objc_framework(path: &str, module: bool) -> Option<&'static str> {
    let (first, rest) = match path.split_once(['/', '.']) {
        Some((first, rest)) => (first, Some(rest)),
        None => (path, None),
    };
    if !module && rest.is_none() {
        return None;
    }
    let framework = OBJC_FRAMEWORKS.iter().find(|name| **name == first)?;
    Some(if *framework == "dispatch" {
        "Dispatch"
    } else {
        framework
    })
}

/// Frameworks an umbrella framework imports: `Cocoa` brings in `Foundation`
/// and `AppKit`, `UIKit` brings in `Foundation`.
pub(crate) fn objc_umbrella_members(framework: &str) -> &'static [&'static str] {
    match framework {
        "Cocoa" => &["Foundation", "AppKit", "CoreData", "CoreFoundation"],
        "AppKit" | "UIKit" => &["Foundation", "CoreFoundation", "CoreGraphics"],
        "Foundation" => &["CoreFoundation", "Dispatch", "objc"],
        _ => &[],
    }
}

/// `NS` classes of AppKit rather than Foundation (`NSView`, `NSWindow`).
const APPKIT_NS_CLASSES: &[&str] = &[
    "NSAlert",
    "NSApp",
    "NSApplication",
    "NSBezierPath",
    "NSButton",
    "NSColor",
    "NSCursor",
    "NSEvent",
    "NSFont",
    "NSImage",
    "NSImageView",
    "NSMenu",
    "NSMenuItem",
    "NSOpenPanel",
    "NSPasteboard",
    "NSResponder",
    "NSSavePanel",
    "NSScreen",
    "NSScrollView",
    "NSStatusBar",
    "NSTableView",
    "NSTextField",
    "NSTextView",
    "NSView",
    "NSViewController",
    "NSWindow",
    "NSWindowController",
    "NSWorkspace",
];

/// Class-name and function-name prefixes of the frameworks.
const OBJC_PREFIXES: &[(&str, &str)] = &[
    ("NS", "Foundation"),
    ("UI", "UIKit"),
    ("CF", "CoreFoundation"),
    ("CG", "CoreGraphics"),
    ("CA", "QuartzCore"),
    ("AV", "AVFoundation"),
    ("WK", "WebKit"),
    ("MK", "MapKit"),
];

/// The framework a class or function belongs to by its prefix: `NSString`
/// and `NSLog` to `Foundation`, `UIView` to `UIKit`, `CGRectMake` to
/// `CoreGraphics`, `dispatch_async` to `Dispatch`. The prefix must be
/// followed by an upper-case letter, so an ordinary identifier that merely
/// starts with the letters (`UInt32`) is not caught.
pub(crate) fn objc_prefix_framework(name: &str) -> Option<&'static str> {
    if name.starts_with("dispatch_") {
        return Some("Dispatch");
    }
    let (prefix, framework) = OBJC_PREFIXES
        .iter()
        .find(|(prefix, _)| name.starts_with(prefix))?;
    if !name[prefix.len()..].starts_with(|ch: char| ch.is_ascii_uppercase()) {
        return None;
    }
    if *framework == "Foundation" && APPKIT_NS_CLASSES.contains(&name) {
        return Some("AppKit");
    }
    Some(framework)
}
