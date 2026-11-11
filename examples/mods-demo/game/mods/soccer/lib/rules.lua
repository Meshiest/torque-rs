-- The soccer rules that need no engine: the pitch's pieces, where the players stand at a kickoff, when the ball is in a goal
-- and how a kick sends it. A module of the mod's own folder, loaded with require("lib.rules").

local rules = {
    ROUND_MS = 60000, -- the length of a round, in the server's simulation milliseconds
    WIN = 3,          -- goals that win a round before its time is up
    GOAL_MS = 2000,   -- the pause after a goal, before the ball is put back on the center spot
    BREAK_MS = 3000,  -- the pause between rounds
    NOTE_MS = 2000,   -- how long the line under the score shows its note (a kickoff, the ball put back)
    TICK_MS = 50,     -- how often the mode runs
    HALF_WIDTH = 8,   -- the pitch inside its walls reaches this far from the middle across (x)
    HALF_LENGTH = 12, -- and this far along (y), to the goal lines
    GOAL_HALF = 3.5,  -- a goal's mouth reaches this far from the middle, between its posts
    GOAL_DEPTH = 1.5, -- how far a goal's net reaches back from the goal line
    GOAL_HEIGHT = 2.1, -- the height of the crossbar's underside
    POST = 0.2,       -- the posts' and the crossbar's thickness
    WALL = 0.5,       -- the walls' thickness
    WALL_HEIGHT = 1.2,
    FLOOR_WIDTH = 22, -- the floor's size, whose picture is drawn for it
    FLOOR_LENGTH = 32,
    -- The floor's top stands a little over the ground the client's view draws round the pitch (at height 0): at the same height
    -- the two would tie in the depth test, and the anti-aliasing's jitter would hand the tie to one, then the other, frame by frame.
    FLOOR_TOP = 0.02,
    FIGURE_HALF = 0.45, -- a player's box reaches this far from its middle across
    KICK_REACH = 0.6, -- how close a player's box must come to the ball to kick it
    KICK_SPEED = 9,   -- the ball's speed off a kick, units a second
    KICK_SPREAD = 0.35, -- a kick goes off up to this far (radians) either side of where it was aimed
    KICK_MS = 600,    -- the least time between two kicks of one player
    LOOSE_MS = 400,   -- the least time between two kicks of the ball, by anyone
    STUCK_MS = 4000,  -- a ball that moves less than STUCK_MOVE for this long is dropped on the center spot again
    STUCK_MOVE = 0.5,
}

-- Every piece of the pitch: its datablock, its middle and its scale. The floor and the walls are one cube scaled; a goal's posts,
-- crossbar and net are shapes of their own size. Blue defends the goal at -y, Red the one at +y.
function rules.pitch()
    local w, l, t, h = rules.HALF_WIDTH, rules.HALF_LENGTH, rules.WALL, rules.WALL_HEIGHT
    local g, d, gh, p = rules.GOAL_HALF, rules.GOAL_DEPTH, rules.GOAL_HEIGHT, rules.POST
    local pieces = {
        { "PitchData", 0, 0, rules.FLOOR_TOP - 0.5, rules.FLOOR_WIDTH, rules.FLOOR_LENGTH, 1 },
        { "WallData", -(w + t / 2), 0, h / 2, t, 2 * (l + t), h },
        { "WallData", w + t / 2, 0, h / 2, t, 2 * (l + t), h },
    }
    local function add(data, x, y, z)
        pieces[#pieces + 1] = { data, x, y, z, 1, 1, 1 }
    end
    for _, goal in ipairs({ { side = -1, team = "Blue" }, { side = 1, team = "Red" } }) do
        local side, team = goal.side, goal.team
        -- The end wall either side of the mouth, from the post out to the side wall's inner face: the side walls run the whole
        -- length, so an end wall that reached into them would share their top and outer faces at the corner and fight them.
        local corner = (w - g - p) / 2
        for _, x in ipairs({ -1, 1 }) do
            pieces[#pieces + 1] = { "WallData", x * (g + p + corner), side * (l + t / 2), h / 2, 2 * corner, t, h }
        end
        local mid = side * (l + p + (d - p) / 2)
        for _, x in ipairs({ -1, 1 }) do
            add(team .. "PostData", x * (g + p / 2), side * (l + p / 2), gh / 2)
            add(team .. "NetSideData", x * (g + p / 2), mid, gh / 2)
        end
        add(team .. "BarData", 0, side * (l + p / 2), gh + p / 2)
        add(team .. "NetBackData", 0, side * (l + d), gh / 2)
        add(team .. "NetRoofData", 0, mid, gh)
    end
    return pieces
end

-- The team (its `attack`: 1 scores at +y, -1 at -y) whose goal the ball at (x, y) is in, or nil: the ball's middle a
-- little past the line, inside the mouth.
function rules.goal(x, y)
    if math.abs(x) > rules.GOAL_HALF or math.abs(y) < rules.HALF_LENGTH + 0.3 then
        return nil
    end
    return y > 0 and 1 or -1
end

-- Whether the ball has left the pitch (over a wall, or through the floor).
function rules.out(x, y, z)
    return math.abs(x) > rules.HALF_WIDTH + 1 or math.abs(y) > rules.HALF_LENGTH + rules.GOAL_DEPTH + 1 or z < -1
end

-- Where a player of the team that attacks `attack` stands at a kickoff, and its yaw: the team that kicks off just behind the
-- ball, the other in front of its own goal.
function rules.kickoff_spot(attack, kicks)
    local yaw = attack > 0 and 0 or math.pi
    if kicks then
        return 0.6, -attack * 1.4, yaw
    end
    return 0, -attack * (rules.HALF_LENGTH - 3), yaw
end

-- A small repeatable random sequence (Park-Miller): the same match every run, on every machine.
local seed = 20240607
function rules.random()
    seed = (seed * 16807) % 2147483647
    return seed / 2147483647
end

local function unit(x, y)
    local d = math.sqrt(x * x + y * y)
    if d < 1e-3 then
        return nil
    end
    return x / d, y / d
end

-- The ball's velocity after a player at (fx, fy) that attacks `attack` kicks the ball at (bx, by) of radius `radius`, or nil
-- when the player's box is not near enough. A kick goes away from the player's middle, turned toward the far post of the goal
-- it attacks unless the player faces the other way, and a little off either side.
function rules.kick(fx, fy, bx, by, radius, attack)
    local gx = math.max(math.abs(bx - fx) - rules.FIGURE_HALF, 0)
    local gy = math.max(math.abs(by - fy) - rules.FIGURE_HALF, 0)
    if math.sqrt(gx * gx + gy * gy) > radius + rules.KICK_REACH then
        return nil
    end
    local ax, ay = unit(bx - fx, by - fy)
    if not ax then
        return nil
    end
    local post = bx > 0 and -1 or 1
    local tx, ty = unit(post * (rules.GOAL_HALF - 1.2) - bx, attack * (rules.HALF_LENGTH + rules.GOAL_DEPTH / 2) - by)
    if tx and ax * tx + ay * ty > -0.5 then
        ax, ay = unit(0.25 * ax + tx, 0.25 * ay + ty)
    end
    local off = (rules.random() * 2 - 1) * rules.KICK_SPREAD
    local c, s = math.cos(off), math.sin(off)
    return (ax * c - ay * s) * rules.KICK_SPEED, (ax * s + ay * c) * rules.KICK_SPEED
end

return rules
