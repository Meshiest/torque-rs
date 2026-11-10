// The example game. The dedicated server execs this file at start-up, then calls loadExampleMission(), and calls
// onClientEnterGame(%client) for every client that connects.

datablock StaticShapeData(ExampleCrateData)
{
   shapeFile = "./shapes/crate.dts";
   category = "Example";
};

datablock ItemData(ExampleGemData)
{
   shapeFile = "./shapes/gem.dts";
   category = "Example";
   rotate = true;
};

datablock CameraData(ExampleObserverData)
{
   mode = "Observer";
};

function loadExampleMission(%file)
{
   exec(%file);
   echo("Mission loaded:" SPC MissionGroup.getCount() SPC "objects");
   new SimGroup(MissionCleanup);
   $Example::GemAngle = 0;
   schedule(100, 0, "spinGem");
}

// The gem circles the crate: a field the host sees change and sends.
function spinGem()
{
   $Example::GemAngle += 0.25;
   ExampleGem.position = mCos($Example::GemAngle) * 4 SPC mSin($Example::GemAngle) * 4 SPC 1.5;
   schedule(100, 0, "spinGem");
}

function onClientEnterGame(%client)
{
   %camera = new Camera()
   {
      dataBlock = ExampleObserverData;
      position = "0 -12 6";
   };
   MissionCleanup.add(%camera);
   echo("Client" SPC %client SPC "entered with camera" SPC %camera);
   return %camera;
}

function onClientLeaveGame(%client, %camera)
{
   if (isObject(%camera))
      %camera.delete();
}
