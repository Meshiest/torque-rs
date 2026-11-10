// The demo's server: a world with nothing in it, and the way a client comes in. The rules are not here: the `soccer` mod is the
// game mode, and replaces GameConnection::onEnterGame and onLeaveGame (empty below) when the server's mods are loaded.

exec("prefs.cs");

// What the network's list of games shows of this server (the game mode keeps the player counts).
$Server::GameType = "Soccer";
$Server::MissionType = "One a team";
$Server::MissionName = "The pitch";
$Server::PlayerCount = 0;
$Pref::Server::MaxPlayers = 8;
if ($Pref::Server::Name $= "")
   $Pref::Server::Name = "Soccer";

datablock CameraData(ObserverData)
{
   mode = "Observer";
};

function startGame()
{
   if (!isObject(MissionCleanup))
      new SimGroup(MissionCleanup);
   new SimGroup(ClientGroup);
}

function GameConnection::onEnterGame(%client)
{
}

function GameConnection::onLeaveGame(%client)
{
}

// With mods the server requires, this runs once the client has the content: `onConnect` waits for it.
function GameConnection::onConnect(%client)
{
   $Server::Connected++;
   ClientGroup.add(%client);
   %client.transmitDataBlocks(1);
}

function GameConnection::onDataBlocksDone(%client, %sequence)
{
   %client.activateGhosting();
   %client.camera = new Camera()
   {
      dataBlock = ObserverData;
      position = "0 -16 12";
   };
   MissionCleanup.add(%client.camera);
   %client.setControlObject(%client.camera);
   %client.onEnterGame();
}

function GameConnection::onDrop(%client, %reason)
{
   %client.onLeaveGame();
   if (isObject(%client.camera))
      %client.camera.delete();
}
