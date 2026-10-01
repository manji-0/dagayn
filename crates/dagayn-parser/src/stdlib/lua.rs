//! The Lua standard library: the library tables (`string`, `table`, `os`)
//! and the base functions in the global table `_G` (`print`, `pairs`).

/// Tables the standard libraries install as globals, which a call names
/// through (`string.format`, `os.time`). `bit32` and `jit`/`ffi` are left
/// out: they belong to single versions and LuaJIT.
const LUA_STD_LIBRARIES: &[&str] = &[
    "coroutine",
    "debug",
    "io",
    "math",
    "os",
    "package",
    "string",
    "table",
    "utf8",
];

/// Functions of the base library, called bare.
const LUA_BASE_FUNCTIONS: &[&str] = &[
    "assert",
    "collectgarbage",
    "dofile",
    "error",
    "getmetatable",
    "ipairs",
    "load",
    "loadfile",
    "loadstring",
    "next",
    "pairs",
    "pcall",
    "print",
    "rawequal",
    "rawget",
    "rawlen",
    "rawset",
    "require",
    "select",
    "setmetatable",
    "tonumber",
    "tostring",
    "type",
    "unpack",
    "xpcall",
];

/// The standard library a table name is: `string`, but not `json`.
pub(crate) fn lua_std_library(name: &str) -> Option<&'static str> {
    LUA_STD_LIBRARIES
        .iter()
        .find(|library| **library == name)
        .copied()
}

pub(crate) fn is_lua_base_function(name: &str) -> bool {
    LUA_BASE_FUNCTIONS.contains(&name)
}
