// A bigger, heavier ball: the soccer mod's shape drawn larger. This file runs after the soccer mod's, so BallData is there to
// copy from.

datablock PropData(BigBallData : BallData)
{
   radius = 0.75;
   drawScale = "1.667 1.667 1.667";
   mass = 3.5;
};
