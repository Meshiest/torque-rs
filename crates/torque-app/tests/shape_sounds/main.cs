// The test game's client: the mixer on, connectTo(%address).

OpenALInitDriver();

function connectTo(%address)
{
   %conn = new GameConnection(ServerConnection);
   %conn.connect(%address);
}
