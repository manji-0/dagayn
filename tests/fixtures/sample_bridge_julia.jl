# Fixture for bridge detection tests (Julia).

function run_command()
    run(`git status`)
end

function read_config()
    open("config.yaml", "r")
end

function write_output()
    write("output.json", "{}")
end

function load_lib()
    Libdl.dlopen("mylib.so")
end

function read_dynamic(path)
    # Dynamic — LOW confidence
    open(path, "r")
end
