-- Where the server's bots run to: the same rules as the headless demo's client bots (src/bot.rs). With the ball in the half a
-- bot attacks (or close in front of it) it runs round behind the ball, then through it toward the goal; with the ball in its
-- own half it keeps goal, in front of its own goal across from the ball. Loaded with require("lib.bots").

local rules = require("lib.rules")

local bots = {
    BEHIND = 1.3, -- how far behind the ball a bot lines up before it runs through it
    CLEAR = 2.5, -- a keeper goes for a ball this near it, when it is between the ball and its own goal
    KEEP_OUT = 2.5, -- a keeper stands this far out from its goal line
    KEEP_ACROSS = 2, -- and at most this far across from the middle
    SPEED = 0.8, -- a bot runs at this share of a player's speed, so a person can beat it to the ball
}

-- A ball this far across from the middle is by a side wall, and one this far along by an end wall: a bot plays it off the
-- wall instead of at the goal.
local WALL_NEAR = rules.HALF_WIDTH - 2.5
local END_NEAR = rules.HALF_LENGTH - 2
-- How far a bot aims from the middle inside the walls: a player's half width short of them.
local REACH_X = rules.HALF_WIDTH - 0.6
local REACH_Y = rules.HALF_LENGTH - 0.6

local function sign(v)
    return v < 0 and -1 or 1
end

local function unit(x, y)
    local l = math.sqrt(x * x + y * y)
    if l < 1e-4 then
        return 0, 0
    end
    return x / l, y / l
end

local function clamp(v, low, high)
    return math.max(low, math.min(high, v))
end

-- Where a bot plays the ball at (x, y) toward, attacking the goal at `attack`: the goal, or off a wall the ball lies by.
local function aim(x, y, attack)
    if math.abs(x) > WALL_NEAR then
        return x - sign(x) * 4, y + attack * 4
    elseif math.abs(y) > END_NEAR and math.abs(x) > rules.GOAL_HALF then
        if y * attack > 0 then
            return 0, y
        end
        return x - sign(x) * 3, y + attack * 4
    end
    return 0, attack * (rules.HALF_LENGTH + 0.5)
end

-- Where a bot at (mx, my) that goes for the ball at (bx, by) runs to: behind it, then through it toward the goal at `attack`.
local function chase(mx, my, bx, by, attack)
    local ax, ay = aim(bx, by, attack)
    local gx, gy = unit(ax - bx, ay - by)
    local behind_x, behind_y = bx - gx * bots.BEHIND, by - gy * bots.BEHIND
    local fx, fy = mx - bx, my - by
    local along = fx * gx + fy * gy
    local across = gx * fy - gy * fx
    if along < -0.5 and math.abs(across) < 0.55 then
        -- Lined up behind it: through the ball toward the goal.
        return bx + gx, by + gy
    elseif along > -0.4 then
        -- Level with the ball or past it: round it, on this side or, by a wall, on the open side.
        local rx, ry = -gy, gx
        local side
        if math.abs(bx) > WALL_NEAR and math.abs(rx) > 0.3 then
            side = -sign(bx) * sign(rx)
        else
            side = across < 0 and -1 or 1
        end
        return behind_x + rx * side * 1.4, behind_y + ry * side * 1.4
    end
    return behind_x, behind_y
end

-- Where a bot at (mx, my) attacking the goal at `attack` (1: at +y, -1: at -y) runs to with the ball at (bx, by), kept
-- inside the walls.
function bots.target(mx, my, bx, by, attack)
    local goal_side = (by - my) * attack > 0.3
    local near = math.sqrt((bx - mx) ^ 2 + (by - my) ^ 2) < bots.CLEAR
    local x, y
    if by * attack >= -0.5 or (goal_side and near) then
        x, y = chase(mx, my, bx, by, attack)
    else
        x, y = clamp(bx * 0.4, -bots.KEEP_ACROSS, bots.KEEP_ACROSS), -attack * (rules.HALF_LENGTH - bots.KEEP_OUT)
    end
    return clamp(x, -REACH_X, REACH_X), clamp(y, -REACH_Y, REACH_Y)
end

-- How many bots play beside `people` players, when at most `wanted` may: one for each free place of the `places`.
function bots.count(wanted, people, places)
    return math.max(0, math.min(wanted, places - people))
end

return bots
