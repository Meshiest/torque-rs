-- Golden orbs: replaces the two functions the orbs mod asks for an orb's worth and its rope's color. This mod loads after
-- orbs, so its definitions sit over orbs' own; with the mod off, the orbs mod's come back.

local GOLD = "255 215 60 255"

local function golden(key)
    return tonumber(key) % 3 == 0
end

ts.define("orbsValue", function(key)
    return golden(key) and 3 or 1
end)

ts.define("orbsBeamColor", function(key, teamColor)
    return golden(key) and GOLD or teamColor
end)

print("golden-orbs: every third orb is worth three")
