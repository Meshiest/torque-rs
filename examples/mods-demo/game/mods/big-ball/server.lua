-- Big ball: replaces the function the soccer mod asks for the ball's datablock at each kickoff. This mod loads after soccer, so
-- its definition sits over soccer's own; with the mod off, the soccer mod's comes back.

ts.define("soccerBallData", function()
    return "BigBallData"
end)

print("big-ball: the match is played with the big ball")
