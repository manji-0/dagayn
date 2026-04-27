// Fixture for bridge detection tests (Go).
package main

import (
	"os/exec"
	"os"
	"plugin"
)

func runCommand() {
	cmd := exec.Command("git", "status")
	_ = cmd
}

func readConfig() ([]byte, error) {
	return os.ReadFile("config.yaml")
}

func writeOutput(data []byte) error {
	return os.WriteFile("output.json", data, 0644)
}

func openFile() (*os.File, error) {
	return os.Open("data/model.bin")
}

func loadPlugin() (*plugin.Plugin, error) {
	return plugin.Open("mylib.so")
}

func readDynamic(path string) ([]byte, error) {
	// Dynamic path — LOW confidence edge
	return os.ReadFile(path)
}
