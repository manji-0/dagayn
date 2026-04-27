/* Fixture for bridge detection tests (C). */
#include <stdio.h>
#include <stdlib.h>
#include <dlfcn.h>

void run_command(void) {
    system("git status");
}

FILE *open_config(void) {
    return fopen("config.yaml", "r");
}

void write_output(void) {
    FILE *f = fopen("output.json", "w");
    fwrite("{}", 1, 2, f);
    fclose(f);
}

void *load_lib(void) {
    return dlopen("mylib.so", RTLD_NOW);
}

void run_dynamic(const char *cmd) {
    /* Dynamic — LOW confidence */
    system(cmd);
}
