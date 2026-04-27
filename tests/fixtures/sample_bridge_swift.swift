// Fixture for bridge detection tests (Swift).
import Foundation

func runProcess() {
    let p = Process.run(URL(fileURLWithPath: "/usr/bin/git"), arguments: ["status"])
    _ = p
}

func loadLib() {
    dlopen("mylib.dylib", RTLD_NOW)
}
