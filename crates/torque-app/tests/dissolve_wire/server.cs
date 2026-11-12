// The test game's server: a camera for each client, a statue (makeStatue) and the sand its dissolve emits.

datablock CameraData(EyeData)
{
   mode = "Observer";
};

datablock StaticShapeData(StatueData)
{
   shapeFile = "art/box.dts";
};

datablock ParticleData(SandParticle)
{
   lifetimeMS = 1500;
   gravityCoefficient = 0.5;
   dragCoefficient = 1;
   colors[0] = "0.9 0.8 0.6 1";
   colors[1] = "0.9 0.8 0.6 0";
   sizes[0] = 0.05;
   sizes[1] = 0.05;
   times[1] = 1;
};

datablock ParticleEmitterData(SandEmitter)
{
   ejectionPeriodMS = 10;
   ejectionVelocity = 0.5;
   thetaMax = 30;
   particles = "SandParticle";
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

// A statue at the origin, kept in $Statue.
function makeStatue()
{
   %o = new StaticShape() { dataBlock = StatueData; };
   %o.setTransform("0 0 1 0 0 1 0");
   MissionCleanup.add(%o);
   $Statue = %o;
   return %o;
}
