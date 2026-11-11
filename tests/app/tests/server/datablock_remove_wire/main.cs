// The test game's client: connectTo(%address), the datablock callbacks counted, and the reason the server dropped it.

function connectTo(%address)
{
   %conn = new GameConnection(ServerConnection);
   %conn.connect(%address);
}

function onDataBlockObjectReceived(%index, %total)
{
   $Received++;
}

function onDataBlocksChanged()
{
   $Changed++;
}

function GameConnection::onConnectionDropped(%this, %reason)
{
   $DropReason = %reason;
}
