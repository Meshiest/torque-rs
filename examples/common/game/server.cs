// What the example games' servers share: the clients' group, a map's loading, a client's way in and out, a figure made for
// it, and the watch for figures that fell off the map. A game's server.cs execs this where it wants ClientGroup made, then
// defines the hooks this calls:
//
//    GameConnection::onClientConnect(%client, %name)   a client connected (before its datablocks are sent)
//    GameConnection::spawnFigure(%client)              its datablocks arrived and its ghosting started: give it a figure
//    GameConnection::onClientEnterGame(%client)        then tell it the game started
//    GameConnection::onClientLeaveGame(%client)        it left (its figure is already gone)
//    GameConnection::onFellOff(%client)                its figure fell below $Server::FallHeight
//
// $Server::MissionSequence counts the maps loaded: a client's datablocks are those of one map, and a client whose datablocks
// came for an older map is not let in.
//
// Credit: the way in (connect, datablocks, ghosting, enter game) and the hooks' names follow TGE's common server scripts as
// Torque3D carries them (MIT: `core/scripts/server/clientConnection.cs` and `missionDownload.cs`, Copyright (c) 2012
// GarageGames, LLC). The code here is the examples' own.

new SimGroup(ClientGroup);

$Server::MissionSequence = 0;
// A figure below this height has fallen off the map.
$Server::FallHeight = -30;

// Loads the map %file (a script that builds MissionGroup) in place of the last one.
function loadMission(%file)
{
   $Server::MissionSequence++;
   $Server::MissionFile = %file;
   if (isObject(MissionGroup))
      MissionGroup.delete();
   if (isObject(MissionCleanup))
      MissionCleanup.delete();
   new SimGroup(MissionCleanup);
   exec(%file);
}

// ---- a client's way in and out

function GameConnection::onConnect(%client, %name)
{
   %client.onClientConnect(%name);
   %client.transmitDataBlocks($Server::MissionSequence);
}

function GameConnection::onDataBlocksDone(%client, %sequence)
{
   if (%sequence != $Server::MissionSequence)
      return;
   %client.activateGhosting();
   %client.spawnFigure();
   %client.onClientEnterGame();
}

function GameConnection::onDrop(%client, %reason)
{
   echo("Client" SPC %client SPC "left:" SPC %reason);
   if (isObject(%client.player))
      %client.player.delete();
   %client.onClientLeaveGame();
}

// A new figure of %dataBlock for the client at %transform (its last one deleted), in MissionCleanup, the client's control
// object.
function GameConnection::createFigure(%client, %dataBlock, %transform)
{
   if (isObject(%client.player))
      %client.player.delete();
   %player = new Player()
   {
      dataBlock = %dataBlock;
      client = %client;
   };
   MissionCleanup.add(%player);
   %player.setTransform(%transform);
   %client.player = %player;
   %client.setControlObject(%player);
   return %player;
}

// ---- falls

// A figure that fell off the map; again every half second once a game started it (`schedule(500, 0, "watchFalls")`).
function watchFalls()
{
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
   {
      %client = ClientGroup.getObject(%i);
      if (isObject(%client.player) && getWord(%client.player.getTransform(), 2) < $Server::FallHeight)
         %client.onFellOff();
   }
   schedule(500, 0, "watchFalls");
}
