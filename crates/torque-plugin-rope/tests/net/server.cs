// The test game's server: a mover for ropes to hang from, a camera for each client, and the commands the tests send.

datablock CameraData(EyeData)
{
   mode = "Observer";
};

datablock RopeData(Cable)
{
   segments = 8;
   thickness = 0.05;
   color = "200 160 90 255";
};

datablock RopeData(Beam)
{
   style = "ribbon";
   segments = 8;
   thickness = 0.1;
   color = "80 200 255 200";
};

// For the pictures: a thick tube, and a glowing ribbon.
datablock RopeData(Thick)
{
   segments = 24;
   thickness = 0.14;
   color = "190 120 60 255";
};

datablock RopeData(Glow)
{
   style = "ribbon";
   segments = 24;
   thickness = 0.3;
   length = 8.3;
   color = "90 220 255 255";
   emissive = true;
};

function startRopes()
{
   new SimGroup(MissionCleanup);
   new SimGroup(ClientGroup);
   MissionCleanup.add(new Camera(Mover) { dataBlock = EyeData; position = "0 0 1"; });
}

function GameConnection::onConnect(%client)
{
   ClientGroup.add(%client);
   %client.transmitDataBlocks(1);
}

function GameConnection::onDataBlocksDone(%client, %sequence)
{
   %client.activateGhosting();
   %client.camera = new Camera() { dataBlock = EyeData; position = "0 -12 2"; };
   MissionCleanup.add(%client.camera);
   %client.setControlObject(%client.camera);
}

function GameConnection::onDrop(%client, %reason)
{
   if (isObject(%client.camera))
      %client.camera.delete();
}

function moveMover(%x)
{
   Mover.setTransform(%x SPC "0 1 0 0 1 0");
}

// A client fires: its own rope is already there, tagged with %id.
function serverCmdFireBeam(%client, %id)
{
   $Fired = ropeNetCreate("0 0 2", "4 0 2", Beam, %id, %client);
   MissionCleanup.add($Fired);
}
