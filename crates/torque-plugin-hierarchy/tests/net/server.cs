// The test game's server: cubes and pistols to hang on each other, and a camera for each client.

datablock CameraData(EyeData)
{
   mode = "Observer";
};

datablock StaticShapeData(CubeData)
{
   shapeFile = "kit/cube.dts";
};

// A shape with nodes off its origin (the muzzle) for children hung on a node.
datablock StaticShapeData(PistolData)
{
   shapeFile = "kit/pistol.dts";
};

// A static item: its setTransform marks its record's position, so a moved root reaches the client, and a child carried
// by its parent would mark it too.
datablock ItemData(BoxData)
{
   shapeFile = "kit/cube.dts";
   mass = 1;
};

function startHierarchy()
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
   %client.camera = new Camera() { dataBlock = EyeData; position = "0 -12 4"; };
   MissionCleanup.add(%client.camera);
   %client.setControlObject(%client.camera);
}

function GameConnection::onDrop(%client, %reason)
{
   if (isObject(%client.camera))
      %client.camera.delete();
}

function cube(%pos, %scale)
{
   %obj = new StaticShape() { dataBlock = CubeData; position = %pos; scale = %scale; };
   MissionCleanup.add(%obj);
   return %obj;
}

function pistol(%pos, %scale)
{
   %obj = new StaticShape() { dataBlock = PistolData; position = %pos; scale = %scale; };
   MissionCleanup.add(%obj);
   return %obj;
}

function box(%pos)
{
   %obj = new Item() { dataBlock = BoxData; position = %pos; static = "1"; rotate = "0"; };
   MissionCleanup.add(%obj);
   return %obj;
}

// A camera no client controls: its record carries its position and yaw as floats, so a client draws it exactly where the
// server has it (a root that moves and turns).
function eye(%pos)
{
   %obj = new Camera() { dataBlock = EyeData; position = %pos; };
   MissionCleanup.add(%obj);
   return %obj;
}

// The ghost index of %obj on the first client's connection (-1 until its first record is acknowledged).
function ghostOf(%obj)
{
   return ClientGroup.getObject(0).getGhostID(%obj);
}
