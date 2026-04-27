<?php
// Fixture for bridge detection tests (PHP).

function runCommand() {
    system("git status");
}

function readConfig() {
    return file_get_contents("config.yaml");
}

function writeOutput() {
    file_put_contents("output.json", "{}");
}

function openFile() {
    return fopen("data/model.bin", "rb");
}

function loadLib() {
    FFI::cdef("", "mylib.so");
}

function readDynamic($path) {
    // Dynamic — LOW confidence
    return file_get_contents($path);
}
