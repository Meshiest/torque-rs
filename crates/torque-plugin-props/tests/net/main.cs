// The test game's client: connectTo(%address) and disconnect().

function connectTo(%address)
{
   %conn = new GameConnection(ServerConnection);
   %conn.connect(%address);
}

function disconnect()
{
   if (isObject(ServerConnection))
      ServerConnection.delete();
}

function GameConnection::onConnectRequestRejected(%this, %why)
{
   $Why = %why;
}

function clientCmdPropRefused(%why)
{
   $Refused++;
   $RefusedWhy = %why;
}
