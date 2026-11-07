// The level demo's server. The host execs this file and then startWorld(%file) with the map the menu chose. A client that
// connects is sent the datablocks; when it has them, its ghosting starts and it is given a figure to walk (the way in and out
// is the examples' shared common/server.cs, which calls this file's hooks).

// Every client is the engine's: the engine's own classes (a map's SpotLights) are ghosted to all.
$Server::EngineClasses = 1;
// A packet to each client every tick (it sends at the slower of this and the client's rate).
$pref::Net::PacketRateToClient = 32;

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

   // Crouched and prone (the engine's own poses, after Torque3D's: its fields, by their names): each pose's box and speeds.
   // The kit's figure has the poses' sequences (crouch_root, prone_root and the rest).
   // Its crouched walk and crawl play faster by the ground covered (they stride 1.2 and 0.75 a second), so no foot slides.
   crouchBoundingBox = "1 1 1.6";
   maxCrouchForwardSpeed = 1.95;
   maxCrouchBackwardSpeed = 1.65;
   maxCrouchSideSpeed = 1.5;
   crouchForce = 3600;
   proneBoundingBox = "2 2 0.8";
   maxProneForwardSpeed = 1.125;
   maxProneBackwardSpeed = 0.9;
   maxProneSideSpeed = 0;
   proneForce = 2400;

   // Swimming (the engine's own pose, after Torque3D's): in water deeper than about two thirds of the swim box's height (the
   // chest) the figure swims, its box long, at its underwater speeds. Lighter than the water, it floats with the surface 1.45
   // up its 2 m box (twice the density), where the treading figure's eye is 15 cm out (the kit's swim_root), and starts to
   // swim not far short of that depth, so the stroke is drawn at the surface from the start.
   density = 0.725;
   swimBoundingBox = "1 2.3 1.8";
   swimForce = 2400;
   maxUnderwaterForwardSpeed = 3;
   maxUnderwaterBackwardSpeed = 2;
   maxUnderwaterSideSpeed = 2;
};

// A hit's reaction over (common/server.cs).
function WorldFigureData::onEndSequence(%this, %obj, %slot)
{
   playerFlinchEnded(%obj, %slot);
}

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
