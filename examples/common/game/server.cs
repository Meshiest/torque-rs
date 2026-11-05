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
// came for an older map is not let in. A map loaded while clients play sends each of them 'MissionEnd' (the sequence that
// ended), then the new map's datablocks; the hooks above run again for it.
//
// Credit: the way in (connect, datablocks, ghosting, enter game) and the hooks' names follow TGE's common server scripts as
// Torque3D carries them (MIT: `core/scripts/server/clientConnection.cs`, `missionDownload.cs` and `missionLoad.cs`,
// Copyright (c) 2012 GarageGames, LLC). The code here is the examples' own.

new SimGroup(ClientGroup);

$Server::MissionSequence = 0;
// A figure below this height has fallen off the map.
$Server::FallHeight = -30;

// Loads the map %file (a script that builds MissionGroup) in place of the last one. The clients already in the game stay
// connected: the last map ends for them (endMission) and they download the new one as they downloaded the first (its
// datablocks, then its ghosts and a new figure).
function loadMission(%file)
{
   endMission();
   $Server::MissionSequence++;
   $Server::MissionFile = %file;
   new SimGroup(MissionCleanup);
   exec(%file);
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
      ClientGroup.getObject(%i).transmitDataBlocks($Server::MissionSequence);
}

// Ends the map that runs, if one does: each client is told (GameConnection::endMission), its figure is gone and its ghosts
// are dropped; then the map's objects are deleted.
function endMission()
{
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
   {
      %client = ClientGroup.getObject(%i);
      %client.endMission();
      if (isObject(%client.player))
         %client.player.delete();
      %client.player = "";
      %client.resetGhosting();
   }
   if (isObject(MissionGroup))
      MissionGroup.delete();
   if (isObject(MissionCleanup))
      MissionCleanup.delete();
}

// The client's map ended (a game's server.cs may tell it more).
function GameConnection::endMission(%client)
{
   commandToClient(%client, 'MissionEnd', $Server::MissionSequence);
}

// ---- a client's way in and out

// %key is the second connect argument: the host's own client sends the key its process gave the server in $Server::HostKey
// (a game that has a host sets it when it starts the server; a dedicated server has none), and that is what marks the
// connection as the host's (%client.isHost). Nothing a client says about itself marks it.
function GameConnection::onConnect(%client, %name, %key)
{
   %client.isHost = $Server::HostKey !$= "" && %key $= $Server::HostKey;
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
   // One watch however often a game starts it (each map it loads may).
   cancel($Server::FallWatch);
   $Server::FallWatch = schedule(500, 0, "watchFalls");
}
