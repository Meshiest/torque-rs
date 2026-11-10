-- Every way out of the sandbox is refused. Sets `$Test::Result` to "ok" when all of them were.

local function refused(what, f, ...)
    local ok = pcall(f, ...)
    assert(not ok, what .. " was allowed")
end

-- No files, processes or environment.
assert(io == nil, "io")
assert(os.execute == nil and os.remove == nil and os.rename == nil and os.exit == nil, "os's process and file functions")
assert(os.getenv == nil and os.tmpname == nil and os.date == nil and os.time == nil, "os keeps only its clock")
assert(type(os.clock()) == "number", "os.clock is there")
assert(dofile == nil and loadfile == nil, "loading files")

-- No code from text and no bytecode.
assert(loadstring == nil and load == nil, "loading text")
assert(string.dump == nil, "string.dump")

-- No debug library, no environments, no package.
assert(debug == nil, "debug")
assert(getfenv == nil and setfenv == nil, "getfenv and setfenv")
assert(package == nil and module == nil, "package")
assert(collectgarbage == nil and gcinfo == nil, "the collector")

-- The shared tables are read-only: no metatable tricks on what other mods see.
refused("rawset on the globals", rawset, _G, "evil", 1)
refused("writing a library", function() string.upper = function() return "pwned" end end)
refused("rawset on a library", rawset, string, "upper", print)
refused("replacing a library's metatable", setmetatable, string, {})
refused("writing the strings' metatable", function() getmetatable("").__index.lower = print end)
refused("rawset on the strings' methods", rawset, getmetatable("").__index, "lower", print)
refused("writing ts", function() ts.call = print end)
refused("setmetatable on ts", setmetatable, ts, { __index = function() return print end })
refused("writing math", function() math.floor = nil end)
refused("writing the os table", function() os.clock = nil end)
local thing = ts.object("Thing")
if thing ~= nil then
    assert(type(getmetatable(thing)) ~= "table", "an object's metatable is hidden")
end

-- Globals a script sets stay in its own table.
evil_global = 1
assert(rawget(_G, "evil_global") == nil, "a global goes to the script's own table, not the shared one")

-- require: only module names of the mod's own folder.
refused("require of a parent folder", require, "../escape")
refused("require of an absolute path", require, "/etc/passwd")
refused("require of a drive", require, "C:\\windows\\x")
refused("require of a dotted parent", require, "..x")
refused("require of something missing", require, "no_such_module")

-- Console functions that run text or touch files.
refused("exec", ts.call, "exec", "main.cs")
refused("eval", ts.call, "eval", "$pwned = 1;")
refused("call", ts.call, "call", "exec", "main.cs")
refused("compile", ts.call, "compile", "main.cs")
refused("fileDelete", ts.call, "fileDelete", "main.cs")
refused("export", ts.call, "export", "$*", "out.cs")
refused("EXEC in another case", ts.call, "EXEC", "main.cs")
refused("scheduling exec", ts.call, "schedule", 0, 0, "exec", "main.cs")
refused("scheduling exec at another position", ts.call, "schedule", 0, 0, "echo", "exec")
refused("a method under its namespace", ts.call, "SimObject::save", "Thing", "out.cs")
refused("call under a namespace", ts.call, "SimObject::call", "Thing", "save", "out.cs")
refused("opening for writing under a namespace", ts.call, "FileObject::openForWrite", "Thing", "out.cs")
refused("exec under a namespace", ts.call, "Foo::exec", "main.cs")
if thing ~= nil then
    refused("save", function() thing:save("out.cs") end)
    refused("scheduling save", function() thing:schedule(0, "save", "out.cs") end)
end
assert(ts.global("$pwned") == "", "nothing ran")

ts.setGlobal("$Test::Result", "ok")
