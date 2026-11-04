// The level demo's server. The host execs this file and then startWorld(%file) with the map the menu chose. A client that
// connects is sent the datablocks; when it has them, its ghosting starts and it is given a figure to walk (the way in and out
// is the examples' shared common/server.cs, which calls this file's hooks).

// ---- datablocks

datablock StaticShapeData(WorldCubeData)
{
   category = "World";
   shapeFile = "kit/cube.dts";
};

datablock PlayerData(WorldFigureData)
{
   category = "World";
   shapeFile = "kit/figure.dts";

   mass = 90;
   drag = 0.3;
   maxDamage = 100;
   boundingBox = "1 1 2";

   runForce = 3600;
   maxForwardSpeed = 6;
   maxBackwardSpeed = 4;
   maxSideSpeed = 5;
   maxStepHeight = 0.6;
   runSurfaceAngle = 60;

   jumpForce = 720;
   jumpDelay = 10;
   minJumpSpeed = 15;
   maxJumpSpeed = 30;
   jumpSurfaceAngle = 70;

   minImpactSpeed = 40;
   maxLookAngle = 1.4;
   minLookAngle = -1.4;
};

// ---- the world

exec("common/server.cs");

// Loads the map %file (a script that builds MissionGroup and a MapInfo object with the title, the spawn point and the yaw).
function startWorld(%file)
{
   loadMission(%file);
   if (!isObject(MapInfo))
      new ScriptObject(MapInfo) { title = fileBase(%file); spawn = "0 0 1"; spawnYaw = 0; };
   $World::Title = MapInfo.title;
   echo("World" SPC $World::Title SPC "with" SPC MissionGroup.getCount() SPC "objects");
   schedule(500, 0, "watchFalls");
}

// ---- a client's way in

function GameConnection::onClientConnect(%client, %name)
{
   echo("Client" SPC %client SPC "connected");
}

function GameConnection::spawnFigure(%client)
{
   $World::Figure = %client.createFigure(WorldFigureData, MapInfo.spawn SPC "0 0 1" SPC MapInfo.spawnYaw);
}

function GameConnection::onClientEnterGame(%client)
{
   commandToClient(%client, 'WorldStarted', $World::Title);
}

function GameConnection::onClientLeaveGame(%client)
{
}

// The client asked to start again at the spawn point.
function serverCmdRespawn(%client)
{
   %client.spawnFigure();
   commandToClient(%client, 'Respawned');
}

// A figure that fell off the map starts again.
function GameConnection::onFellOff(%client)
{
   %client.spawnFigure();
   commandToClient(%client, 'Fell');
}
