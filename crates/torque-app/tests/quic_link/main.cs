// The test game's client: connectTo(%address), and what the server answered.

function connectTo(%address)
{
   %conn = new GameConnection(ServerConnection);
   %conn.connect(%address);
}

function GameConnection::onConnectionAccepted(%this)
{
   $Accepted++;
}

function GameConnection::onConnectRequestRejected(%this, %why)
{
   $Rejected = %why;
}

function leave()
{
   if (isObject(ServerConnection))
      ServerConnection.delete();
}
