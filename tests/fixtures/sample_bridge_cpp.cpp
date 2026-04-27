// Fixture for bridge detection tests (C++).
#include <fstream>
#include <cstdlib>

void run_command() {
    std::system("git status");
}

void open_file() {
    std::ifstream in("config.yaml");
}

void write_output() {
    std::ofstream out("output.json");
    out << "{}";
}
