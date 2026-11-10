-- The soccer game mode. It runs on the server only: a client is told the score and the clock, steers its own player and sees what
-- the server ghosts to it, and never sees this file run (a server cannot make a client run code).
--
-- Each player who enters joins the team with fewer players and gets a player on the pitch. The ball is a prop: a player's box
-- pushes it, and a player that comes up to it kicks it toward the goal it attacks (rules.kick). A ball in a goal scores for the
-- team that attacks it; after a short pause both players go back to their places and the team that conceded kicks off. A ball
-- that leaves the pitch or sticks in a corner is put back on the center spot. A round ends at WIN goals or when its clock runs
-- out; the result is shown for a moment and the next round starts at 0 - 0.

local rules = require("lib.rules")

local TEAMS = {
    { name = "Blue", data = "BlueFigureData", attack = 1 },
    { name = "Red", data = "RedFigureData", attack = -1 },
}

local players = {} -- client id -> { figure, team, kicked }
local score = { 0, 0 }
local round = 1
local phase = "play" -- "play", "goal" (the pause after one) or "over" (between rounds)
local phaseEnds = 0
local roundEnds = nil
local ball, radius = nil, 0
local kicksOff = 1 -- the team that kicks off next
local message = ""
local goals, kicks = 0, 0
local lastKick = nil -- when the ball was last kicked
local rest = nil -- where the ball was when it last moved, and when: { x, y, t }
local kickedOff = 0 -- when the last kickoff was
local ticks = 0

-- The datablock of the ball each kickoff spawns. A mod that loads later replaces this function (a definition in a later mod's
-- package sits over this one's), which is how big-ball changes the match without touching this file.
ts.define("soccerBallData", function()
    return "BallData"
end)

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

local function numbers(text)
    local out = {}
    for word in string.gmatch(text or "", "%S+") do
        out[#out + 1] = tonumber(word)
    end
    return out
end

local function ball_position()
    local t = numbers(ts.call("propGetTransform", ball))
    return t[1], t[2], t[3]
end

local function figure_position(player)
    local object = ts.object(player.figure)
    if not object then
        return nil
    end
    local p = numbers(object:getPosition())
    return p[1], p[2]
end

local function place(player)
    local team = TEAMS[player.team]
    local x, y, yaw = rules.kickoff_spot(team.attack, player.team == kicksOff)
    ts.call("soccerPlace", player.figure, x, y, yaw)
end

local function kickoff()
    if ball then
        ts.call("propDelete", ball)
    end
    local spawned = numbers(ts.call("soccerBall", ts.call("soccerBallData")))
    ball, radius = spawned[1], spawned[2]
    for _, id in ipairs(sorted_ids()) do
        place(players[id])
    end
    rest = nil
    kickedOff = now()
    phase = "play"
end

local function start_round()
    score = { 0, 0 }
    roundEnds = now() + rules.ROUND_MS
    kicksOff = round % 2 == 1 and 1 or 2
    message = "Round " .. round .. " - kickoff"
    kickoff()
end

local function end_round()
    phase = "over"
    phaseEnds = now() + rules.BREAK_MS
    local winner = ""
    if score[1] > score[2] then
        winner = TEAMS[1].name
    elseif score[2] > score[1] then
        winner = TEAMS[2].name
    end
    message = winner == "" and "A draw" or (winner .. " wins the round")
    ts.setGlobal("$Soccer::Winner", winner)
end

local function scored(attack)
    local team = attack == TEAMS[1].attack and 1 or 2
    score[team] = score[team] + 1
    goals = goals + 1
    kicksOff = 3 - team
    phase = "goal"
    phaseEnds = now() + rules.GOAL_MS
    message = "Goal for " .. TEAMS[team].name .. "!"
end

-- A player whose box comes near the ball kicks it (rules.kick), once each KICK_MS; the ball flies free for LOOSE_MS after a
-- kick, so two players at it never cancel each other's kicks.
local function kick(t, bx, by)
    if lastKick and t - lastKick < rules.LOOSE_MS then
        return
    end
    for _, id in ipairs(sorted_ids()) do
        local player = players[id]
        local fx, fy = figure_position(player)
        if fx and t - (player.kicked or -rules.KICK_MS) >= rules.KICK_MS then
            local vx, vy = rules.kick(fx, fy, bx, by, radius, TEAMS[player.team].attack)
            if vx then
                ts.call("propSetVelocity", ball, vx .. " " .. vy .. " 0")
                ts.call("soccerKickSound", bx, by, radius)
                player.kicked = t
                lastKick = t
                kicks = kicks + 1
                return
            end
        end
    end
end

local function play(t)
    local x, y, z = ball_position()
    if not x then
        kickoff()
        return
    end
    local attack = rules.goal(x, y)
    if attack then
        scored(attack)
    elseif rules.out(x, y, z) then
        message = "The ball went out - kickoff"
        kickoff()
    elseif rest and t - rest.t > rules.STUCK_MS then
        message = "The ball is stuck - kickoff"
        kickoff()
    else
        if not rest or math.abs(x - rest.x) + math.abs(y - rest.y) > rules.STUCK_MOVE then
            rest = { x = x, y = y, t = t }
        end
        kick(t, x, y)
    end
end

local function publish()
    if not roundEnds then
        return
    end
    local left = math.max(0, math.ceil(((phase == "over" and phaseEnds or roundEnds) - now()) / 1000))
    local text = table.concat({ round, phase, left, score[1], score[2], message }, "\t")
    for _, id in ipairs(sorted_ids()) do
        ts.call("soccerAnnounce", id, text)
    end
    ts.setGlobal("$Soccer::Score", TEAMS[1].name .. " " .. score[1] .. " " .. TEAMS[2].name .. " " .. score[2])
    ts.setGlobal("$Soccer::Round", round)
    ts.setGlobal("$Soccer::Phase", phase)
    ts.setGlobal("$Soccer::Goals", goals)
    ts.setGlobal("$Soccer::Kicks", kicks)
    ts.setGlobal("$Soccer::Ball", ts.call("soccerBallData"))
end

ts.define("soccerTick", function()
    local t = now()
    -- The first round starts at the first tick, once every mod is loaded: a later mod's soccerBallData is in place by then.
    if not roundEnds then
        start_round()
    end
    if phase == "play" then
        if t - kickedOff > rules.NOTE_MS then
            message = ""
        end
        play(t)
        if phase == "play" and t >= roundEnds then
            end_round()
        end
    elseif phase == "goal" and t >= phaseEnds then
        if math.max(score[1], score[2]) >= rules.WIN or t >= roundEnds then
            end_round()
        else
            message = TEAMS[kicksOff].name .. " kicks off"
            kickoff()
        end
    elseif phase == "over" and t >= phaseEnds then
        round = round + 1
        start_round()
    end
    ticks = ticks + 1
    if ticks % 2 == 0 then
        publish()
    end
    ts.call("schedule", rules.TICK_MS, 0, "soccerTick")
end)

ts.define("GameConnection::onEnterGame", function(client)
    local id = tonumber(client)
    if players[id] then
        return
    end
    local count = { 0, 0 }
    for _, player in pairs(players) do
        count[player.team] = count[player.team] + 1
    end
    local team = count[2] < count[1] and 2 or 1
    local player = { figure = tonumber(ts.call("soccerFigure", id, TEAMS[team].data)), team = team }
    players[id] = player
    place(player)
    ts.call("soccerTeam", id, TEAMS[team].name, TEAMS[team].attack)
    print("soccer: " .. TEAMS[team].name .. " enters (client " .. id .. ")")
    publish()
end)

ts.define("GameConnection::onLeaveGame", function(client)
    local id = tonumber(client)
    local player = players[id]
    if not player then
        return
    end
    players[id] = nil
    local object = ts.object(player.figure)
    if object then
        object:delete()
    end
    print("soccer: " .. TEAMS[player.team].name .. " leaves")
end)

for _, piece in ipairs(rules.pitch()) do
    ts.call("soccerBlock", table.unpack(piece))
end
print("soccer: the game mode is loaded in the " .. ts.realm .. " realm")
ts.call("schedule", rules.TICK_MS, 0, "soccerTick")
