-- The client-script realm's escape suite: every way out is refused or absent. Logs one line per case that got through
-- ("ALLOWED <case>") and, last, "escape: <n> refused, <m> allowed".

local refusedCount, allowed = 0, 0

local function pass()
    refusedCount += 1
end

local function fail(what)
    allowed += 1
    realm.log("ALLOWED " .. what)
end

-- `f(...)` must raise an error.
local function refused(what, f, ...)
    if pcall(f, ...) then fail(what) else pass() end
end

-- `value` must be nil.
local function absent(what, value)
    if value == nil then pass() else fail(what) end
end

-- `f(...)` must answer false (a request the realm turns down without an error).
local function declined(what, f, ...)
    local ok, result = pcall(f, ...)
    if ok and result == false then pass() else fail(what) end
end

-- No files, processes, environment, code from text or bytecode.
absent("io", io)
absent("os.execute", os.execute)
absent("os.getenv", os.getenv)
absent("os.time", os.time)
absent("os.date", os.date)
absent("os.remove", os.remove)
absent("os.exit", os.exit)
absent("load", load)
absent("loadstring", loadstring)
absent("dofile", dofile)
absent("loadfile", loadfile)
absent("string.dump", string.dump)
absent("debug", debug)
absent("getfenv", getfenv)
absent("setfenv", setfenv)
absent("collectgarbage", collectgarbage)
absent("gcinfo", gcinfo)
absent("package", package)
absent("module", module)

-- No bridge into the engine: no ts, no console, no objects, no globals by name.
absent("ts", ts)
absent("the console", rawget(_G, "console"))
absent("SERVER", SERVER)
absent("CLIENT", CLIENT)

-- Read-only built-ins and the realm's own tables.
refused("rawset on the globals", rawset, _G, "evil", 1)
refused("writing a library", function() string.upper = function() return "pwned" end end)
refused("replacing a library's metatable", setmetatable, string, {})
refused("writing the strings' metatable", function() getmetatable("").__index.lower = print end)
refused("writing math", function() math.floor = nil end)
refused("writing realm", function() realm.log = print end)
refused("rawset on realm", rawset, realm, "time", print)
refused("writing ui", function() ui.new = print end)
refused("setmetatable on ui", setmetatable, ui, { __index = function() return print end })
refused("writing net", function() net.send = print end)
refused("rawset on net", rawset, net, "send", print)
refused("writing print's table through _G", function() _G.print = nil end)
evil_global = 1
if rawget(_G, "evil_global") == nil then pass() else fail("a global reaching the shared table") end

-- __metatable tricks on handles.
local root = ui.root()
local panel = ui.new("panel", {}, root)
if type(getmetatable(panel)) ~= "table" then pass() else fail("a control's metatable is reachable") end
refused("setmetatable on a control", setmetatable, panel, {})
refused("a table with a control's metatable", function()
    local fake = setmetatable({}, getmetatable(panel))
    ui.new("panel", {}, fake)
end)

-- require stays within the mod.
refused("require of a parent folder", require, "../escape")
refused("require of an absolute path", require, "/etc/passwd")
refused("require of a drive", require, "C:\\windows\\x")
refused("require with a NUL", require, "lib\0x")
refused("require of a dotted parent", require, "..x")
refused("require of another mod's file", require, "otherlib")
refused("require of a script by its file name", require, "main.lua")

-- Content paths: only plain paths of mods, images only.
local function image(path)
    return function() ui.new("image", { image = path }) end
end
refused("an image path with ..", image("mods/x/../../config/prefs.cs"))
refused("a prefs file", image("config/prefs.cs"))
refused("a script file as an image", image("mods/x/client/main.lua"))
refused("an image on a drive", image("C:/x.png"))
refused("an image path with backslashes", image("mods\\x\\a.png"))
refused("an image path with a NUL", image("mods/x/a.png\0.lua"))
refused("an absolute image path", image("/mods/x/a.png"))
refused("an image elsewhere in the cache", image("cache/0123abcd/logo.png"))
refused("an image named after a device", image("mods/x/con.png"))
refused("an image under a folder of only dots", image("mods/x/.../a.png"))
refused("an image under a folder with a trailing dot", image("mods/x/a./b.png"))
refused("text with a line separator", ui.new, "text", { text = "a\u{2028}b" })
refused("text with a direction override", ui.new, "text", { text = "a\u{202e}b" })

-- Handle forgery.
refused("a number as a parent", ui.new, "panel", {}, 1)
refused("a string as a parent", ui.new, "panel", {}, "PlayGui")
refused("a table as a parent", ui.new, "panel", {}, { id = 1 })
local doomed = ui.new("panel", {})
doomed:destroy()
refused("a destroyed control's set", function() doomed:set({ visible = false }) end)
refused("a destroyed control as a parent", ui.new, "panel", {}, doomed)
refused("destroying a root", function() root:destroy() end)
refused("moving a root", function() root:parent(panel) end)
local child = ui.new("panel", {}, panel)
refused("a control under its own child", function() panel:parent(child) end)
refused("focus of a number", ui.focus, 5)
refused("focus of a button", ui.focus, ui.new("button", { text = "x" }))
refused("a timer as a parent", ui.new, "panel", {}, realm.after(10, function() end))

-- Unknown kinds and props, and the fields that run code.
refused("a window", ui.new, "window")
refused("a console line", ui.new, "GuiConsoleEditCtrl")
refused("a 3D view", ui.new, "GameTSCtrl")
for _, field in { "command", "altCommand", "variable", "accelerator", "class", "superClass", "profile", "name",
    "closeCommand", "validate", "escapeCommand", "expression", "bitmap" } do
    refused("the field " .. field, ui.new, "button", { [field] = "$ClientScript::Canary = 1;" })
end
refused("a prop of another kind", ui.new, "button", { rows = { "a" } })
refused("a prop name that is not a string", ui.new, "panel", { [1] = true })

-- Values out of range or of the wrong type.
refused("a font size of 200", ui.new, "text", { fontSize = 200 })
refused("an opacity of 2", ui.new, "panel", { opacity = 2 })
refused("an infinite position", ui.new, "panel", { pos = { math.huge, 0 } })
refused("a NaN size", ui.new, "panel", { size = { 0 / 0, 10 } })
refused("a max length of 1000", ui.new, "textfield", { maxLength = 1000 })
refused("an unknown style", ui.new, "button", { style = "evil" })
refused("text of 5000 bytes", ui.new, "text", { text = string.rep("a", 5000) })
refused("text with an escape character", ui.new, "text", { text = "\27[31mred" })
refused("text that is not UTF-8", ui.new, "text", { text = "\255\254" })
refused("a selection past the rows", ui.new, "list", { rows = { "a" }, selected = 2 })
refused("a slider value outside its range", ui.new, "slider", { range = { 0, 1 }, value = 5 })
refused("a color out of range", ui.new, "panel", { fill = { 2, 0, 0 } })
refused("a fractional max length", ui.new, "textfield", { maxLength = 1.5 })

-- Rich text: links, images and fonts by name are not in the markup.
refused("a link in rich text", ui.new, "richtext", { text = "<a:gotoWebPage>click</a>" })
refused("an image by name in rich text", ui.new, "richtext", { text = "<bitmap:base/client/ui/x>" })
refused("a font by name in rich text", ui.new, "richtext", { text = "<font:Arial Bold:24>big" })

-- The tree's caps.
refused("controls nested 33 deep", function()
    local at = ui.root()
    for _ = 1, 33 do
        at = ui.new("panel", {}, at)
    end
end)

-- Focus needs a recent click of the realm's own controls (none yet).
declined("focus without user activation", ui.focus, ui.new("textfield", {}))

-- No Escape handler, no keys, no cursor, no clipboard.
refused("an Escape handler", ui.on, "escape", function() end)
refused("a key handler on a control", function() panel:on("key", function() end) end)
refused("a key handler on the realm", realm.on, "key", function() end)
absent("ui.cursor", ui.cursor)
absent("ui.setCursor", ui.setCursor)
absent("ui.clipboard", ui.clipboard)
absent("realm.clipboard", realm.clipboard)
absent("ui.keys", ui.keys)

-- net reaches its own server only, with plain values.
refused("net.send with an address", net.send, "hi", "203.0.113.5:28000")
refused("net.send of a function", net.send, print)
refused("net.send of a control", net.send, panel)
refused("net.send of a coroutine", net.send, coroutine.create(function() end))
refused("net.send of a buffer", net.send, buffer.create(4))
local cycle = {}
cycle.self = cycle
refused("net.send of a cycle", net.send, cycle)
refused("net.send past 4 KiB", net.send, string.rep("x", 5000))
refused("net.send of a table with a function key", net.send, { [print] = 1 })
refused("net.call of an address", net.call, "203.0.113.5:28000", {}, function() end)
refused("net.call of a URL", net.call, "http://example.com", {}, function() end)
refused("net.call without a function", net.call, "props.spawn", {}, nil)
absent("net.connect", net.connect)
absent("net.http", net.http)
absent("net.url", net.url)
absent("net.socket", net.socket)

-- Tiers this realm was not granted, or this build does not have.
absent("the state table", state)
absent("the draw table", draw)
absent("the effects table", effects)
absent("the input table", input)
absent("the camera table", camera)
absent("the world table", world)
absent("the predict table", predict)

-- Timers within their rules.
refused("a negative timer", realm.after, -1, function() end)
refused("a NaN timer", realm.every, 0 / 0, function() end)
refused("cancelling a control", realm.cancel, panel)

realm.log(`escape: {refusedCount} refused, {allowed} allowed`)
