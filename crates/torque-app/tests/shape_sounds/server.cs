// The test game's server: a camera for each client, a box that plays sounds in its slots (makeSpeaker), and two profiles of the
// same sound, one looping.

datablock CameraData(EyeData)
{
   mode = "Observer";
};

datablock StaticShapeData(SpeakerData)
{
   shapeFile = "art/box.dts";
};

datablock AudioDescription(PingDescription)
{
   volume = 1;
   is3D = true;
   referenceDistance = 2;
   maxDistance = 50;
   type = 0;
};

datablock AudioDescription(HumDescription : PingDescription)
{
   isLooping = true;
};

datablock AudioProfile(Ping)
{
   fileName = "sounds/ping.wav";
   description = PingDescription;
};

datablock AudioProfile(Hum)
{
   fileName = "sounds/ping.wav";
   description = HumDescription;
};

function startIt()
{
   new SimGroup(MissionCleanup);
   new SimGroup(ClientGroup);
}

function GameConnection::onConnect(%client)
{
   ClientGroup.add(%client);
   %client.transmitDataBlocks(1);
}

function GameConnection::onDataBlocksDone(%client, %sequence)
{
   %client.activateGhosting();
   %client.camera = new Camera() { dataBlock = EyeData; position = "0 -8 2"; };
   MissionCleanup.add(%client.camera);
   %client.setControlObject(%client.camera);
   $Connected++;
}

function GameConnection::onDrop(%client, %reason)
{
   if (isObject(%client.camera))
      %client.camera.delete();
}

// A speaker at %x %y %z, kept in $Speaker.
function makeSpeaker(%x, %y, %z)
{
   %o = new StaticShape() { dataBlock = SpeakerData; };
   %o.setTransform(%x SPC %y SPC %z SPC "0 0 1 0");
   MissionCleanup.add(%o);
   $Speaker = %o;
   return %o;
}
