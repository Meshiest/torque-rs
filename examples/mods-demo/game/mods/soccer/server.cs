// The soccer mod's datablocks and the few helpers its Lua rules call: Lua cannot declare a datablock or make an object of its
// own, so the declarations and the `new` calls are here, and the rules (server.lua) are the game mode.

// The floor and the walls: one cube each, scaled to every piece of the pitch (lib/rules.lua lists them and places the goals).
datablock StaticShapeData(PitchData) { category = "Soccer"; shapeFile = "shapes/pitch.dts"; };
datablock StaticShapeData(WallData) { category = "Soccer"; shapeFile = "shapes/wall.dts"; };

// Each team's goal: white posts with a band of its color, a crossbar, and a net tinted with its color, each piece solid so a
// ball in the goal stays in.
datablock StaticShapeData(BluePostData) { category = "Soccer"; shapeFile = "shapes/post_blue.dts"; };
datablock StaticShapeData(BlueBarData) { category = "Soccer"; shapeFile = "shapes/bar_blue.dts"; };
datablock StaticShapeData(BlueNetBackData) { category = "Soccer"; shapeFile = "shapes/net_back_blue.dts"; };
datablock StaticShapeData(BlueNetSideData) { category = "Soccer"; shapeFile = "shapes/net_side_blue.dts"; };
datablock StaticShapeData(BlueNetRoofData) { category = "Soccer"; shapeFile = "shapes/net_roof_blue.dts"; };
datablock StaticShapeData(RedPostData) { category = "Soccer"; shapeFile = "shapes/post_red.dts"; };
datablock StaticShapeData(RedBarData) { category = "Soccer"; shapeFile = "shapes/bar_red.dts"; };
datablock StaticShapeData(RedNetBackData) { category = "Soccer"; shapeFile = "shapes/net_back_red.dts"; };
datablock StaticShapeData(RedNetSideData) { category = "Soccer"; shapeFile = "shapes/net_side_red.dts"; };
datablock StaticShapeData(RedNetRoofData) { category = "Soccer"; shapeFile = "shapes/net_roof_red.dts"; };

// A player. It runs as fast every way, so steering needs no turning.
datablock PlayerData(BlueFigureData)
{
   category = "Soccer";
   shapeFile = "shapes/player_blue.dts";
   mass = 90;
   drag = 0.3;
   maxDamage = 100;
   boundingBox = "0.9 0.9 1.8";
   runForce = 5400;
   maxForwardSpeed = 6;
   maxBackwardSpeed = 6;
   maxSideSpeed = 6;
   maxStepHeight = 0.3;
   runSurfaceAngle = 60;
   jumpForce = 0;
   maxLookAngle = 1.4;
   minLookAngle = -1.4;
};

datablock PlayerData(RedFigureData : BlueFigureData)
{
   shapeFile = "shapes/player_red.dts";
};

// The ball: a prop the players' boxes push, which the rules kick when a player runs into it.
datablock PropData(BallData)
{
   shapeFile = "shapes/ball.dts";
   radius = 0.45;
   mass = 2;
   friction = 0.4;
   restitution = 0.7;
   linearDamping = 0.5;
   angularDamping = 0.5;
   maxSpeed = 24;
   canFreeze = false;
};

function soccerBlock(%data, %x, %y, %z, %sx, %sy, %sz)
{
   if (!isObject(Pitch))
      new SimGroup(Pitch);
   %block = new StaticShape()
   {
      dataBlock = %data;
      position = %x SPC %y SPC %z;
      scale = %sx SPC %sy SPC %sz;
   };
   Pitch.add(%block);
   return %block;
}

// A new player of %data for %client, the client's control object.
function soccerFigure(%client, %data)
{
   if (!isObject(MissionCleanup))
      new SimGroup(MissionCleanup);
   %player = new Player()
   {
      dataBlock = %data;
      client = %client;
   };
   MissionCleanup.add(%player);
   %client.player = %player;
   %client.setControlObject(%player);
   return %player;
}

// How many bots may play when nobody has taken their places (one player a team); a server's prefs or command line may
// set it before the mod loads.
if ($Pref::Soccer::Bots $= "")
   $Pref::Soccer::Bots = 2;

// A bot: a player of %data that no client steers. The rules steer it (soccerSteer).
function soccerBot(%data)
{
   if (!isObject(MissionCleanup))
      new SimGroup(MissionCleanup);
   %bot = new AIPlayer()
   {
      dataBlock = %data;
   };
   MissionCleanup.add(%bot);
   return %bot;
}

// The bot %bot runs to %x %y at %speed of its full speed, looking up the pitch from %from toward the goal it attacks
// (%attack: 1 at +y, -1 at -y), so it faces that goal as a player does.
function soccerSteer(%bot, %x, %y, %from, %attack, %speed)
{
   %bot.setMoveSpeed(%speed);
   %bot.setMoveDestination(%x SPC %y SPC 0, false);
   %bot.setAimLocation(%from SPC %attack * 40 SPC 1.6);
}

function soccerHold(%bot)
{
   %bot.stop();
}

// A bot that reaches where it was sent waits for its next order (the rules send it somewhere at every step).
function PlayerData::onReachDestination(%this, %bot)
{
}

function PlayerData::onMoveStuck(%this, %bot)
{
}

function soccerPlace(%player, %x, %y, %yaw)
{
   %player.setTransform(%x SPC %y SPC "0.05 0 0 1" SPC %yaw);
   %player.setVelocity("0 0 0");
}

// A ball of %data on the center spot; its radius too, which the rules need for the goal line.
function soccerBall(%data)
{
   return propSpawn(%data, "0 0" SPC %data.radius + 0.02) SPC %data.radius;
}

// The ball was kicked at %x %y %z: every player hears the kick there (the client plays the mod's sound as a 3D sound,
// quieter the farther away it is).
function soccerKickSound(%x, %y, %z)
{
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
      commandToClient(ClientGroup.getObject(%i), 'PlaySound', "sounds/kick.wav", %x, %y, %z);
}

function soccerTeam(%client, %name, %attack)
{
   commandToClient(%client, 'SoccerTeam', %name, %attack);
}

function soccerAnnounce(%client, %state)
{
   commandToClient(%client, 'SoccerState', %state);
}
