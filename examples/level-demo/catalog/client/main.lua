-- The prop catalog's menu. It runs on the player's machine, in a sandbox, and only if the player allowed it. It shows the props the
-- server offers, each a button with its picture and name, and a field for an optional label. The server decides what is put down;
-- this file only asks and shows.
--
-- What it sends:     hello                      (show me the catalog)
--                    spawn|kind|label           (put one of these down in front of me)
--                    clear                      (take back my props)
-- What it gets:      catalog|kind|name|kind|name...      what can be spawned
--                    recent|player|kind|label|...        the last spawns, oldest first
--                    reply|ok|text  or  reply|refused|reason
--                    toggle                              the player pressed the game's key for the menu
-- The bar is the only separator. (A server's script never sees a control character in what a player sends, so a tab or a newline
-- could not be one.)
--
-- Escape closes the menu: the engine tells a menu that held the cursor `closed` (the first Escape press), and the game's own key
-- brings it back through the server's `toggle`. A menu binds no keys of its own.

local ICON_DIR = "mods/catalog/icons/"
local CARD_W, CARD_H, GAP = 98, 124, 8
local SHOWN_RECENT = 5

local INK = "#eef0f6"
local MUTED = "#9aa3b8"
local BAD = "#ff8a80"

local BOARD_W, BOARD_H = 758, 628
local INNER = BOARD_W - 24

local root = ui.root()

-- A small button that shows or hides the board, so the player can see the office.
local toggle = ui.new("button", { text = "Hide catalog", size = { 120, 28 }, pos = { 8, 8 } }, root)

local board = ui.new("panel", {
    pos = { 8, 44 },
    size = { BOARD_W, BOARD_H },
    fill = "#151a26ee",
    border = "#34405e",
    radius = 10,
}, root)
local page = ui.new("flex", {
    size = { BOARD_W, BOARD_H },
    direction = "column",
    gap = { 6, 6 },
    padding = 12,
}, board)

-- The top row: the title, then the label for the next spawn.
local head = ui.new("flex", { size = { INNER, 34 }, direction = "row", gap = { 12, 12 }, alignItems = "center" }, page)
ui.new("text", { text = "Prop catalog", font = "title", fontSize = 24, color = INK, size = { 180, 32 } }, head)
ui.new("text", { text = "Label", fontSize = 15, color = MUTED, size = { 44, 24 } }, head)
local label = ui.new("textfield", {
    size = { 300, 30 },
    placeholder = "Optional, for example Bob's chair",
    maxLength = 40,
}, head)
local clear = ui.new("button", { text = "Clear my props", size = { 120, 30 } }, head)

-- The catalog: a grid of buttons, each the whole card (a picture and a name). It is built when the server sends the list.
local ROWS = 3
local grid = ui.new("grid", {
    size = { INNER, ROWS * CARD_H + (ROWS - 1) * GAP },
    columns = { CARD_W, CARD_W, CARD_W, CARD_W, CARD_W, CARD_W, CARD_W },
    autoFlow = "row",
    gap = { GAP, GAP },
}, page)
local cards = {}
-- The label last sent: the field is cleared when the server takes the spawn, unless the player has typed another since.
local sent = nil

-- Only what the server refused is shown here.
local status = ui.new("text", { text = "", fontSize = 15, color = BAD, size = { INNER, 20 } }, page)

ui.new("text", { text = "Recent spawns", font = "title", fontSize = 17, color = INK, size = { INNER, 20 } }, page)
local recent_lines = {}
for i = 1, SHOWN_RECENT do
    recent_lines[i] = ui.new("text", { text = "", fontSize = 14, color = MUTED, size = { INNER, 16 } }, page)
end

local function say(text)
    status:set({ text = text })
end

-- Sends a request; a word to the player when the engine holds it back.
local function ask(text)
    if net.send(text) then
        say("")
    else
        say("Could not send that. Try again in a moment.")
    end
end

local function build_cards(entries)
    for _, card in ipairs(cards) do
        card:destroy()
    end
    cards = {}
    for _, entry in ipairs(entries) do
        local card = ui.new("button", {
            text = entry.name,
            image = ICON_DIR .. entry.kind .. ".png",
            size = { CARD_W, CARD_H },
            fontSize = 13,
        }, grid)
        card:on("press", function()
            sent = label:get("text")
            ask("spawn|" .. entry.kind .. "|" .. sent)
        end)
        cards[#cards + 1] = card
    end
end

local function show_recent(rows)
    -- The newest first, as many as there is room for.
    for i = 1, SHOWN_RECENT do
        local row = rows[#rows + 1 - i]
        local text = ""
        if row then
            text = row.player .. " put down a " .. string.lower(row.kind)
            if row.label ~= "" then
                text = text .. ": " .. row.label
            end
        end
        recent_lines[i]:set({ text = text, color = i == 1 and INK or MUTED })
    end
end

local open = true
local function show(on)
    open = on
    board:set({ visible = open })
    toggle:set({ text = open and "Hide catalog" or "Show catalog" })
end

toggle:on("press", function()
    show(not open)
end)

clear:on("press", function()
    ask("clear")
end)

-- Escape: the engine took the cursor back and says so.
ui.on("closed", function()
    show(false)
end)

-- What the server says. The bar is the only separator, as it is in what the menu sends.
local function split(text, separator)
    local parts = {}
    for part in (text .. separator):gmatch("(.-)" .. separator) do
        parts[#parts + 1] = part
    end
    return parts
end

net.on(function(message)
    if type(message) ~= "string" then
        return
    end
    if message == "toggle" then
        show(not open)
        -- The cursor is the menu's only while the menu is open.
        ui.interactive(open)
        return
    end
    local parts = split(message, "|")
    if parts[1] == "catalog" then
        local entries = {}
        for i = 2, #parts - 1, 2 do
            entries[#entries + 1] = { kind = parts[i], name = parts[i + 1] }
        end
        build_cards(entries)
    elseif parts[1] == "recent" then
        local rows = {}
        for i = 2, #parts - 2, 3 do
            rows[#rows + 1] = { player = parts[i], kind = parts[i + 1], label = parts[i + 2] }
        end
        show_recent(rows)
    elseif parts[1] == "reply" then
        if parts[2] == "ok" then
            say("")
            if sent ~= nil and label:get("text") == sent then
                label:set({ text = "" })
            end
        else
            say(parts[3] or "The server said no.")
        end
    end
end)

ui.interactive(true)
net.send("hello")
