// The test game's server: a camera for each client, a crate as a StaticShape and the same crate as a plugin's Prop.

datablock CameraData(EyeData)
{
   mode = "Observer";
};

datablock StaticShapeData(CrateStatic)
{
   shapeFile = "art/crate.dts";
};

// The crate as a heat haze: drawn as the bend alone (the engine protocol's distortion fields).
datablock StaticShapeData(CrateHaze)
{
   shapeFile = "art/crate.dts";
   distortion = 0.25;
   distortionEdge = 2;
   distortionTint = "0.9 0.95 1 0.5";
   distortionOnly = true;
};

datablock PropData(CrateProp)
{
   model = "art/crate.dts";
};

datablock PropData(CrateResting)
{
   model = "art/crate.dts";
   rests = "1";
};

function startProps()
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
   %client.camera = new Camera() { dataBlock = EyeData; position = "0 -12 2"; };
   MissionCleanup.add(%client.camera);
   %client.setControlObject(%client.camera);
}

function GameConnection::onDrop(%client, %reason)
{
   if (isObject(%client.camera))
      %client.camera.delete();
}

function makeStatic(%xf, %data)
{
   if (%data $= "")
      %data = CrateStatic;
   %o = new StaticShape() { dataBlock = %data; };
   %o.setTransform(%xf);
   MissionCleanup.add(%o);
   return %o;
}

function makeProp(%xf, %data)
{
   if (%data $= "")
      %data = CrateProp;
   %o = new Prop() { dataBlock = %data.getId(); };
   %o.setTransform(%xf);
   MissionCleanup.add(%o);
   return %o;
}

// A plugin class's own mask bits are the plugin's to mark; the scale call marks bit 1 of any scene object.
function moveProp(%o, %xf)
{
   %o.setTransform(%xf);
   %o.setScale("1 1 1");
}
