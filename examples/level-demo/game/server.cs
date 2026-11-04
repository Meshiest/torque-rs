// The level demo's server. The host execs this file and then startWorld(%file) with the map the menu chose. A client that
// connects is sent the datablocks; when it has them, its ghosting starts and it is given a figure to walk.

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

new SimGroup(ClientGroup);

$World::Sequence = 0;
// A figure below this height has fallen off the map and starts again.
$World::FallHeight = -30;

// Loads the map %file (a script that builds MissionGroup and a MapInfo object with the title, the spawn point and the yaw).
function startWorld(%file)
{
   $World::Sequence++;
   $World::File = %file;
   if (isObject(MissionGroup))
      MissionGroup.delete();
   if (isObject(MissionCleanup))
      MissionCleanup.delete();
   new SimGroup(MissionCleanup);
   exec(%file);
   if (!isObject(MapInfo))
      new ScriptObject(MapInfo) { title = fileBase(%file); spawn = "0 0 1"; spawnYaw = 0; };
   $World::Title = MapInfo.title;
   echo("World" SPC $World::Title SPC "with" SPC MissionGroup.getCount() SPC "objects");
   schedule(500, 0, "watchFalls");
}

// ---- a client's way in

function GameConnection::onConnect(%client, %name)
{
   echo("Client" SPC %client SPC "connected");
   %client.transmitDataBlocks($World::Sequence);
}

function GameConnection::onDataBlocksDone(%client, %sequence)
{
   if (%sequence != $World::Sequence)
      return;
   %client.activateGhosting();
   %client.spawnFigure();
   commandToClient(%client, 'WorldStarted', $World::Title);
}

function GameConnection::spawnFigure(%client)
{
   if (isObject(%client.player))
      %client.player.delete();
   %player = new Player()
   {
      dataBlock = WorldFigureData;
   };
   MissionCleanup.add(%player);
   %player.setTransform(MapInfo.spawn SPC "0 0 1" SPC MapInfo.spawnYaw);
   %client.player = %player;
   %client.setControlObject(%player);
   $World::Figure = %player;
}

function GameConnection::onDrop(%client, %reason)
{
   echo("Client" SPC %client SPC "left:" SPC %reason);
   if (isObject(%client.player))
      %client.player.delete();
}

// The client asked to start again at the spawn point.
function serverCmdRespawn(%client)
{
   %client.spawnFigure();
   commandToClient(%client, 'Respawned');
}

// A figure that fell off the map starts again.
function watchFalls()
{
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
   {
      %client = ClientGroup.getObject(%i);
      if (isObject(%client.player) && getWord(%client.player.getTransform(), 2) < $World::FallHeight)
      {
         %client.spawnFigure();
         commandToClient(%client, 'Fell');
      }
   }
   schedule(500, 0, "watchFalls");
}
