// The test game's server: a camera to ghost from, a client that gets its datablocks at once and then ghosts from the camera,
// and the chain's callbacks counted.

datablock CameraData(EyeData) { mass = 1; };

function startIt()
{
   new SimGroup(ClientGroup);
   new Camera(Eye) { dataBlock = EyeData; position = "0 0 0"; };
}

function GameConnection::onConnect(%client)
{
   ClientGroup.add(%client);
   $Client = %client;
   %client.transmitDataBlocks(1);
}

function GameConnection::onDataBlocksDone(%client, %seq)
{
   $Done++;
   %client.setControlObject(Eye);
   %client.activateGhosting();
}

function GameConnection::onDataBlocksUpdated(%client, %count)
{
   $Updated++;
}
