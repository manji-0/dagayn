// Fixture for bridge detection tests (C#).
using System.IO;
using System.Diagnostics;
using System.Reflection;

class BridgeSamples
{
    void RunProcess()
    {
        Process.Start("git", "status");
    }

    string ReadConfig()
    {
        return File.ReadAllText("config.yaml");
    }

    void WriteOutput()
    {
        File.WriteAllText("output.json", "{}");
    }

    void LoadLib()
    {
        Assembly.LoadFile("mylib.dll");
    }

    string ReadDynamic(string path)
    {
        // Dynamic — LOW confidence
        return File.ReadAllText(path);
    }
}
