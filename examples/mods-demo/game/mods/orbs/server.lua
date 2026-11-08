-- The orbs game mode. It runs on the server only: a client is told the round's state and draws what the server ghosts to it, and
-- never sees this file run (a server cannot make a client run code).
--
-- Each player who enters gets a runner that chases the nearest orb and a rope from the runner to it. Taking an orb scores
-- orbsValue() points and puts a new one on the field. The leader hangs from a gold cable. A round lasts ROUND_MS; then the
-- scores are shown for a moment and the next round starts over.

local rules = require("lib.rules")

local TEAMS = {
    { data = "RunnerAData", color = "90 150 255 255", name = "Blue" },
    { data = "RunnerBData", color = "255 110 110 255", name = "Red" },
}

local players = {} -- client id -> { runner, rope, x, y, score, target, team }
local orbs = {}    -- key -> { id, x, y, value }
local joined = 0
local nextKey = 1
local round = 1
local phase = "play"
local phaseEnds = nil
local lastTick = nil
local ticks = 0
local crown = nil
local leader = nil
local collected = 0
local points = 0

local function now()
    return tonumber(ts.call("getSimTime"))
end

local function sorted_ids()
    local ids = {}
    for id in pairs(players) do
        ids[#ids + 1] = id
    end
    table.sort(ids)
    return ids
end

local function count(t)
    local n = 0
    for _ in pairs(t) do
        n = n + 1
    end
    return n
end

-- What an orb is worth and the color its rope takes. A mod that loads later replaces these two functions (a definition in a
-- later mod's package sits over this one's), which is how golden-orbs changes the rules without touching this file.
ts.define("orbsValue", function(key)
    return 1
end)

ts.define("orbsBeamColor", function(key, teamColor)
    return teamColor
end)

local function add_orb()
    local x, y = rules.place()
    local key = nextKey
    nextKey = nextKey + 1
    local id = tonumber(ts.call("orbsSpawn", "OrbData", x, y, rules.HEIGHT, 1))
    orbs[key] = { id = id, x = x, y = y, value = tonumber(ts.call("orbsValue", key)) }
end

-- Points the runner at the nearest orb, and its rope with it.
local function aim(player)
    local key = rules.nearest(orbs, player.x, player.y)
    if key == player.target then
        return
    end
    player.target = key
    if key then
        ts.call("ropeNetAttach", player.rope, "end", orbs[key].id)
        ts.call("ropeNetSetColor", player.rope, ts.call("orbsBeamColor", key, player.team.color))
    end
end

local function take(player, key)
    local orb = orbs[key]
    orbs[key] = nil
    local object = ts.object(orb.id)
    if object then
        object:delete()
    end
    player.score = player.score + orb.value
    collected = collected + 1
    points = points + orb.value
end

local function publish()
    phaseEnds = phaseEnds or (now() + rules.ROUND_MS)
    local ids = sorted_ids()
    local lines, scores = {}, {}
    for _, id in ipairs(ids) do
        lines[#lines + 1] = players[id].team.name .. " " .. players[id].score
        scores[#scores + 1] = players[id].team.name .. ":" .. players[id].score
    end
    local left = math.max(0, math.ceil((phaseEnds - now()) / 1000))
    local text = round .. "\t" .. phase .. "\t" .. left .. "\t" .. table.concat(lines, "\t")
    for _, id in ipairs(ids) do
        ts.call("orbsAnnounce", id, text)
    end
    ts.setGlobal("$Orbs::Scores", table.concat(scores, " "))
    ts.setGlobal("$Orbs::Round", round)
    ts.setGlobal("$Orbs::Phase", phase)
    ts.setGlobal("$Orbs::Collected", collected)
    ts.setGlobal("$Orbs::Points", points)
end

local function place_runner(player, slot, total)
    local angle = (slot - 1) / math.max(total, 2) * 2 * math.pi + 0.8
    player.x, player.y = math.cos(angle) * 6, math.sin(angle) * 6
    player.target = nil
    ts.call("orbsMove", player.runner, player.x, player.y, rules.HEIGHT)
end

local function start_round()
    phase = "play"
    phaseEnds = now() + rules.ROUND_MS
    local ids = sorted_ids()
    for slot, id in ipairs(ids) do
        players[id].score = 0
        place_runner(players[id], slot, #ids)
    end
end

-- The runner with the most points hangs from the crown's cable; nobody does until somebody scores.
local function update_leader()
    local best, bestScore = nil, 0
    for _, id in ipairs(sorted_ids()) do
        if players[id].score > bestScore then
            best, bestScore = id, players[id].score
        end
    end
    if best == leader then
        return
    end
    leader = best
    if crown == nil then
        crown = tonumber(ts.call("orbsRope", "OrbCrown"))
        ts.call("ropeNetPin", crown, "end", "0 2 5")
    end
    if best then
        ts.call("ropeNetAttach", crown, "start", players[best].runner)
    else
        ts.call("ropeNetPin", crown, "start", "0 2 5")
    end
end

local function run(dt)
    for _, id in ipairs(sorted_ids()) do
        local player = players[id]
        aim(player)
        local orb = orbs[player.target]
        if orb then
            player.x, player.y = rules.step(player.x, player.y, orb.x, orb.y, rules.SPEED * dt)
            ts.call("orbsMove", player.runner, player.x, player.y, rules.HEIGHT)
            if rules.distance(player.x, player.y, orb.x, orb.y) < rules.REACH then
                take(player, player.target)
            end
        end
    end
    update_leader()
end

ts.define("orbsTick", function()
    local t = now()
    phaseEnds = phaseEnds or (t + rules.ROUND_MS)
    local dt = math.min((t - (lastTick or t)) / 1000, 0.5)
    lastTick = t
    if phase == "play" and t >= phaseEnds then
        phase = "break"
        phaseEnds = t + rules.BREAK_MS
        ts.setGlobal("$Orbs::Winner", leader and players[leader].team.name or "")
    elseif phase == "break" and t >= phaseEnds then
        round = round + 1
        start_round()
    end
    if phase == "play" then
        while count(orbs) < rules.ORBS do
            add_orb()
        end
        run(dt)
    end
    ticks = ticks + 1
    if ticks % 5 == 0 then
        publish()
    end
    ts.call("schedule", rules.TICK_MS, 0, "orbsTick")
end)

ts.define("GameConnection::onEnterGame", function(client)
    local id = tonumber(client)
    joined = joined + 1
    local team = TEAMS[(joined - 1) % #TEAMS + 1]
    local rope = tonumber(ts.call("orbsRope", "OrbBeam"))
    local player = {
        runner = tonumber(ts.call("orbsSpawn", team.data, 0, 0, rules.HEIGHT, 0)),
        rope = rope,
        score = 0,
        team = team,
    }
    ts.call("ropeNetSetColor", rope, team.color)
    ts.call("ropeNetAttach", rope, "start", player.runner)
    players[id] = player
    place_runner(player, count(players), count(players))
    print("orbs: " .. team.name .. " enters (client " .. id .. ")")
    publish()
end)

ts.define("GameConnection::onLeaveGame", function(client)
    local id = tonumber(client)
    local player = players[id]
    if not player then
        return
    end
    players[id] = nil
    if leader == id then
        leader = nil
    end
    for _, handle in ipairs({ player.runner, player.rope }) do
        local object = ts.object(handle)
        if object then
            object:delete()
        end
    end
    print("orbs: " .. player.team.name .. " leaves")
end)

print("orbs: the game mode is loaded in the " .. ts.realm .. " realm")
ts.call("schedule", rules.TICK_MS, 0, "orbsTick")
