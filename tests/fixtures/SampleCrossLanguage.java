// Fixture for cross-language bridge detection tests (Java).
// Only canonical receiver forms are detected; aliased variables require
// dataflow resolution and are intentionally not detected.

public class SampleCrossLanguage {

    public void invokeViaGetRuntime() throws Exception {
        Runtime.getRuntime().exec("./target/release/dagayn-core");
    }

    public void invokeViaRuntimeExec() throws Exception {
        Runtime.exec("./scripts/build.sh");
    }

    public void loadSystemLibrary() {
        System.loadLibrary("dagayn");
    }

    public void loadSystemLibraryPath() {
        System.load("./target/release/libdagayn.so");
    }

    public void loadViaGetRuntime() {
        Runtime.getRuntime().loadLibrary("helper");
    }

    // Dynamic argument — should produce LOW confidence
    public void invokeDynamic(String cmd) throws Exception {
        Runtime.getRuntime().exec(cmd);
    }
}
