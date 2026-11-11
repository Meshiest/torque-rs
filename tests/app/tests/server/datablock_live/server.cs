// The test game's server: a client gets its datablocks at once, and the chain's callbacks are counted.

datablock AudioDescription(DescA) { is3D = true; referenceDistance = 1.5; };
datablock AudioDescription(DescB) { is3D = true; referenceDistance = 2.5; };
datablock AudioDescription(DescC) { is3D = true; referenceDistance = 3.5; };

function startIt()
{
   new SimGroup(ClientGroup);
}

function GameConnection::onConnect(%client)
{
   ClientGroup.add(%client);
   $Client = %client;
   $Connected++;
   %client.transmitDataBlocks(1);
}

function GameConnection::onDataBlocksDone(%client, %seq)
{
   $Done++;
}

function GameConnection::onDataBlocksUpdated(%client, %count)
{
   $Updated++;
   $Counts = $Counts @ %count @ ",";
}
