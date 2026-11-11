// The server's script: the engine runs this file, then calls startGame().

function startGame()
{
   echo("Server up");
}

// Called for each client that connects.
function GameConnection::onConnect(%client)
{
   echo("Player joined");
   %client.transmitDataBlocks(1);
}

function GameConnection::onDataBlocksDone(%client, %sequence)
{
   %client.activateGhosting();
}

function GameConnection::onDrop(%client, %reason)
{
   echo("Player left");
}
