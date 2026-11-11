// The test game's server: a camera for each client, so the client gets a ghost.

datablock CameraData(EyeData)
{
   mode = "Observer";
};

function startIt()
{
   new SimGroup(MissionCleanup);
   new SimGroup(ClientGroup);
}

function GameConnection::onConnect(%client)
{
   ClientGroup.add(%client);
   %client.transmitDataBlocks(1);
}

function GameConnection::onDataBlocksDone(%client, %sequence)
{
   %client.activateGhosting();
   %client.camera = new Camera() { dataBlock = EyeData; position = "0 -8 2"; };
   MissionCleanup.add(%client.camera);
   %client.setControlObject(%client.camera);
   $Connected++;
}

function GameConnection::onDrop(%client, %reason)
{
   if (isObject(%client.camera))
      %client.camera.delete();
}
