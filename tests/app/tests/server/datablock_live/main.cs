// The test game's client: connectTo(%address), and the datablock callbacks counted.

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
