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

// The figures' pose triggers (Torque3D's globals; the engine reads them for the datablocks' crouched and prone poses): Ctrl
// held is the move's trigger 3, Z's prone the trigger 1 (the clients' keys, common/client.cs). Trigger 4 is the arena's
// aim and 5 the buggy's way out, so neither is Torque3D's prone or sprint here.
$player::crouchTrigger = 3;
$player::proneTrigger = 1;

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
// object, named as the client is (%client.name).
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
   // The name the other players see over it (cleanName).
   if (%client.name !$= "")
      %player.setShapeName(%client.name);
   %client.player = %player;
   %client.setControlObject(%player);
   return %player;
}

// ---- names and chat
//
// What a client says of itself (the name it connects with, the lines it sends) is cleaned before anything shows it: its
// control bytes (0x01 to 0x1F and 0x7F) are taken out first, so none is left to hide a markup tag from the next step; then
// its markup tags (StripMLControlChars, until none is left: no color tags or line breaks of its own, so a player's line
// cannot pass for the game's); then the spaces at either end are trimmed and it is cut to $Server::NameBytes (a name) or
// $Server::ChatBytes (a line), never in the middle of a character. StripMLControlChars keeps only about the first 63 bytes
// of a line with many `<` in it (a line with none is kept whole). The lines go to every client as data ('ChatMessage',
// common/chat.cs), never into a script.
//
// The lines are counted by the address a client sends from (its IP, without the port number), so every connection from
// one address shares one allowance (the host's own connection has its own) and a reconnect does not start a new one:
// $Server::ChatBurst lines at once, then one more each $Server::ChatRefillMs. A line past that is dropped and its
// connection told once. An address's allowance is forgotten once it is full again (a new one would allow as much); at most
// $Server::ChatAddresses are kept, and a line from one more address is dropped.
$Server::NameBytes = 24;
$Server::ChatBytes = 120;
$Server::ChatBurst = 4;
$Server::ChatRefillMs = 1500;
$Server::ChatAddresses = 256;
if (!isObject(ChatAllowances))
   new SimGroup(ChatAllowances);

// The bytes taken out before the markup (every control byte a string can hold, and DEL).
$Server::ControlBytes = "\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18" @
   "\x19\x1a\x1b\x1c\x1d\x1e\x1f\x7f";

// The bytes that continue a UTF-8 character (0x80 to 0xBF): a cut before one of them moves back to the character's start.
$Server::Utf8Tail = "\x80\x81\x82\x83\x84\x85\x86\x87\x88\x89\x8a\x8b\x8c\x8d\x8e\x8f\x90\x91\x92\x93\x94\x95\x96\x97" @
   "\x98\x99\x9a\x9b\x9c\x9d\x9e\x9f\xa0\xa1\xa2\xa3\xa4\xa5\xa6\xa7\xa8\xa9\xaa\xab\xac\xad\xae\xaf\xb0\xb1\xb2\xb3" @
   "\xb4\xb5\xb6\xb7\xb8\xb9\xba\xbb\xbc\xbd\xbe\xbf";

// %text without control bytes or markup, trimmed, at most %max bytes.
function cleanText(%text, %max)
{
   %text = trim(StripMLControlChars(stripChars(%text, $Server::ControlBytes)));
   %n = strlen(%text);
   if (%n <= %max)
      return %text;
   %n = %max;
   while (%n > 0 && strstr($Server::Utf8Tail, getSubStr(%text, %n, 1)) >= 0)
      %n--;
   return trim(getSubStr(%text, 0, %n));
}

// The name %client goes by: the one it connected with, cleaned ("Player" and its number when nothing is left).
function cleanName(%name, %client)
{
   %name = cleanText(%name, $Server::NameBytes);
   return %name $= "" ? "Player" SPC %client : %name;
}

// %kind is "chat" (a player's line: %a is the name, %b the text) or "info" (a line from the game: %a).
function messageAll(%kind, %a, %b)
{
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
      commandToClient(ClientGroup.getObject(%i), 'ChatMessage', %kind, %a, %b);
}

// The address %client sends from without its port ("IP:1.2.3.4:28000" is "IP:1.2.3.4"; "local" as it is); the host's own
// connection has an allowance of its own ("host"), whatever address it comes from.
function GameConnection::chatAddress(%client)
{
   if (%client.isHost)
      return "host";
   %address = %client.getAddress();
   %cut = -1;
   for (%at = strpos(%address, ":"); %at >= 0; %at = strpos(%address, ":", %at + 1))
      %cut = %at;
   return %cut > 0 ? getSubStr(%address, 0, %cut) : %address;
}

// Whether %client may send a line now (one taken from its address's allowance when it may).
function GameConnection::chatAllowed(%client)
{
   %now = getSimTime();
   %address = %client.chatAddress();
   %allowance = 0;
   for (%i = ChatAllowances.getCount() - 1; %i >= 0; %i--)
   {
      %a = ChatAllowances.getObject(%i);
      %a.left = getMin($Server::ChatBurst, %a.left + (%now - %a.at) / $Server::ChatRefillMs);
      %a.at = %now;
      if (%a.address $= %address)
         %allowance = %a;
      else if (%a.left >= $Server::ChatBurst)
         %a.delete();
   }
   if (!isObject(%allowance))
   {
      if (ChatAllowances.getCount() >= $Server::ChatAddresses)
         return false;
      %allowance = new ScriptObject()
      {
         address = %address;
         left = $Server::ChatBurst;
         at = %now;
      };
      ChatAllowances.add(%allowance);
   }
   if (%allowance.left < 1)
      return false;
   %allowance.left--;
   return true;
}

function serverCmdSay(%client, %text)
{
   %text = cleanText(%text, $Server::ChatBytes);
   if (%text $= "")
      return;
   if (!%client.chatAllowed())
   {
      if (!%client.chatWarned)
         commandToClient(%client, 'ChatMessage', "info", "Too many messages. Wait a moment, then try again.");
      %client.chatWarned = true;
      return;
   }
   %client.chatWarned = false;
   messageAll("chat", %client.name, %text);
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

// ---- hit reactions
//
// A hit's reaction on a figure (the kit figure's flinch blends): away from the side the shot came from (%travel, the way the
// shot flew), the head snapped back for a head shot (%part "head") and a punch to the body with a small step otherwise. The
// server plays it as a script thread, so its hitboxes have it and every client draws it from the thread its record carries;
// two threads (2 and 3) take turns, so a reaction still playing goes on while the next starts, and each is stopped once over
// (playerFlinchEnded, from the datablock's onEndSequence), so the next one on it starts from its start on every client too.
// The view of the figure's own client does not move (its first-person eye is steady).
$Flinch::Ms = 320;
function playerFlinch(%obj, %travel, %part)
{
   if (!isObject(%obj) || %obj.getState() $= "Dead")
      return;
   %fwd = %obj.getForwardVector();
   %right = getWord(%fwd, 1) SPC -getWord(%fwd, 0) SPC 0;
   // The side the shot came from: against the way it flew.
   %ahead = -VectorDot(%travel, %fwd);
   %side = -VectorDot(%travel, %right);
   if (mAbs(%ahead) >= mAbs(%side))
      %dir = %ahead >= 0 ? "front" : "back";
   else
      %dir = %side >= 0 ? "right" : "left";
   %name = (%part $= "head" ? "flinch_head_" : "flinch_") @ %dir;
   %slot = %obj.flinchSlot == 2 ? 3 : 2;
   // The same reaction still playing on that thread is left to play (a client would not start it again).
   if (%obj.flinchName[%slot] $= %name && getSimTime() < %obj.flinchUntil[%slot])
      return;
   %obj.flinchSlot = %slot;
   %obj.flinchName[%slot] = %name;
   %obj.flinchUntil[%slot] = getSimTime() + $Flinch::Ms;
   %obj.playThread(%slot, %name);
}

// A reaction's thread over: stopped (at its start, at rest).
function playerFlinchEnded(%obj, %slot)
{
   if (%slot == 2 || %slot == 3)
   {
      %obj.stopThread(%slot);
      %obj.flinchName[%slot] = "";
   }
}
