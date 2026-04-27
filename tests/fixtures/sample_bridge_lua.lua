-- Fixture for bridge detection tests (Lua).

local function run_command()
    os.execute("git status")
end

local function open_config()
    return io.open("config.yaml", "r")
end

local function write_output()
    local f = io.open("output.json", "w")
    f:write("{}")
    f:close()
end

local function load_lib()
    package.loadlib("mylib.so", "luaopen_mylib")
end

local function run_dynamic(cmd)
    -- Dynamic — LOW confidence
    os.execute(cmd)
end
