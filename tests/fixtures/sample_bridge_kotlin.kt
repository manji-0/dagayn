// Fixture for bridge detection tests (Kotlin).
import java.io.File
import java.nio.file.Files

fun runCommand() {
    Runtime.getRuntime().exec("git status")
}

fun readConfig(): String {
    return Files.readString(java.nio.file.Path.of("config.yaml"))
}

fun writeOutput() {
    Files.writeString(java.nio.file.Path.of("output.json"), "{}")
}

fun loadLib() {
    System.loadLibrary("mylib")
}
