-- The orbs rules that need no engine: where orbs appear, which is nearest, how a runner steps. A module of the mod's own folder,
-- loaded with require("lib.rules").

local rules = {
    ROUND_MS = 14000, -- the length of a round, in the server's simulation milliseconds
    BREAK_MS = 2500,  -- the pause between rounds
    TICK_MS = 100,    -- how often the mode runs
    ORBS = 5,         -- orbs on the field at once
    FIELD = 8,        -- the field reaches this far from the middle each way
    SPEED = 6.0,      -- a runner's speed, units a second
    REACH = 0.7,      -- how close a runner must come to take an orb
    HEIGHT = 0.7,     -- the height the orbs and the runners' middles are at
}

-- A small repeatable random sequence (Park-Miller): the same game every run, on every machine.
local seed = 20240607
function rules.random()
    seed = (seed * 16807) % 2147483647
    return seed / 2147483647
end

-- A place for an orb, at least 2 units from the middle.
function rules.place()
    local x, y
    repeat
        x = (rules.random() * 2 - 1) * rules.FIELD
        y = (rules.random() * 2 - 1) * rules.FIELD
    until x * x + y * y > 4
    return x, y
end

function rules.distance(ax, ay, bx, by)
    local dx, dy = bx - ax, by - ay
    return math.sqrt(dx * dx + dy * dy)
end

-- The orb nearest to (x, y), as its key in `orbs`, or nil when there is none.
function rules.nearest(orbs, x, y)
    local best, bestDistance
    for key, orb in pairs(orbs) do
        local d = rules.distance(x, y, orb.x, orb.y)
        if bestDistance == nil or d < bestDistance or (d == bestDistance and key < best) then
            best, bestDistance = key, d
        end
    end
    return best
end

-- One step of `distance` from (x, y) toward (tx, ty), stopping at it.
function rules.step(x, y, tx, ty, distance)
    local d = rules.distance(x, y, tx, ty)
    if d <= distance then
        return tx, ty
    end
    return x + (tx - x) / d * distance, y + (ty - y) / d * distance
end

return rules
