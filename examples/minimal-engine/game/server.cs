// The server's way in: the minimal example's game (example/main.cs: its datablocks and loadExampleMission), and for each client
// that connects its datablocks, ghosting, and the camera the game makes for it as its control object.

exec("example/main.cs");

// The game moves the gem by writing its position field, which the hand-wired server notices by comparing fields; the engine's server
// sends a ghost update when a transform is set, so the gem moves with setTransform.
function spinGem()
{
   $Example::GemAngle += 0.25;
   ExampleGem.setTransform(mCos($Example::GemAngle) * 4 SPC mSin($Example::GemAngle) * 4 SPC 1.5 SPC "1 0 0 0");
   schedule(100, 0, "spinGem");
}

function GameConnection::onConnect(%client)
{
   %client.transmitDataBlocks(1);
}

function GameConnection::onDataBlocksDone(%client, %sequence)
{
   %client.activateGhosting();
   %client.camera = onClientEnterGame(%client);
   %client.setControlObject(%client.camera);
}

function GameConnection::onDrop(%client, %reason)
{
   onClientLeaveGame(%client, %client.camera);
}
