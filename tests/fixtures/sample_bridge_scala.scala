// Fixture for bridge detection tests (Scala).
import java.nio.file.Files
import java.nio.file.Path

object BridgeSamples {
  def runCommand(): Unit = {
    Runtime.getRuntime().exec("git status")
  }

  def readConfig(): String = {
    Files.readString(Path.of("config.yaml"))
  }

  def writeOutput(): Unit = {
    Files.writeString(Path.of("output.json"), "{}")
  }

  def loadLib(): Unit = {
    System.loadLibrary("mylib")
  }
}
