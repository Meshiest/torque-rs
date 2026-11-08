-- The API surface, run in a server realm. The host first runs this TorqueScript:
--   new ScriptObject(Thing) { health = 10; };
--   function tsDouble(%x) { return %x * 2; }
-- and afterwards checks `$Test::Result` is "ok", then calls the functions this script defines.

assert(ts.realm == "server", "realm: " .. tostring(ts.realm))
assert(SERVER == true and CLIENT == false)

-- Console functions: arguments and results are text.
assert(ts.call("strupr", "abc") == "ABC")
assert(ts.call("tsDouble", 21) == "42")
assert(ts.call("getWordCount", "a b c") == "3")
assert(ts.call("strlen", 1.5) == "3", "a number is passed as a script writes it")
assert(ts.call("noSuchFunctionAnywhere") == "")

-- Globals.
ts.setGlobal("$Test::FromLua", 7)
assert(ts.global("$Test::FromLua") == "7")
assert(ts.global("Test::FromLua") == "7", "the $ is optional")

-- Objects: fields, methods, ids.
local thing = ts.object("Thing")
assert(thing ~= nil, "Thing is found by its name")
assert(thing.health == "10")
thing.health = 25
assert(thing.health == "25")
assert(ts.field(thing, "health") == "25")
assert(ts.setField(thing, "armor", 3))
assert(thing.armor == "3")
assert(thing:getName() == "Thing")
assert(ts.method(thing, "getName") == "Thing")
assert(type(thing.id) == "number")
assert(ts.object(thing.id) == thing, "found again by its id")
assert(ts.object(tostring(thing.id)) == thing)
assert(ts.exists(thing))
assert(ts.object("NoSuchObject") == nil)
assert(not pcall(function() thing.getName() end), "a method needs the colon")

-- Errors are Lua errors.
local ok, message = pcall(function() ts.call("strupr", {}) end)
assert(not ok and string.find(tostring(message), "cannot be passed"), tostring(message))

-- Callbacks: console functions and methods TorqueScript calls.
ts.define("luaAdd", function(a, b) return a + b end)
ts.define("ScriptObject::luaGreet", function(this, who)
    return "hello " .. who .. " from " .. ts.object(this):getName()
end)
ts.define("luaNothing", function() end)

print("basics", "ran", 3)
ts.setGlobal("$Test::Result", "ok")
