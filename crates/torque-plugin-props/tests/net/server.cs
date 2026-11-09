// The test game's server: a floor 400 units square with its top at z = 0, a camera for each client (or, with $Test::Players,
// a figure to walk), and the kinds of prop the tests spawn.

datablock CameraData(EyeData)
{
   mode = "Observer";
};

datablock StaticShapeData(FloorData)
{
   shapeFile = "kit/cube.dts";
};

datablock PlayerData(FigureData)
{
   shapeFile = "kit/figure.dts";
   mass = 90;
   drag = 0.3;
   maxDamage = 100;
   boundingBox = "1 1 2";
   runForce = 3600;
   maxForwardSpeed = 6;
   maxBackwardSpeed = 4;
   maxSideSpeed = 5;
   maxStepHeight = 0.6;
   runSurfaceAngle = 60;
   jumpForce = 720;
   jumpSurfaceAngle = 70;
   minImpactSpeed = 40;
};

datablock WheeledVehicleTire(BuggyTire)
{
   shapeFile = "kit/buggy_wheel.dts";
   staticFriction = 4.4;
   kineticFriction = 3.4;
   lateralForce = 17000;
   lateralDamping = 4000;
   lateralRelaxation = 1;
   longitudinalForce = 17000;
   longitudinalDamping = 4000;
   longitudinalRelaxation = 1;
   radius = 0.5;
};

datablock WheeledVehicleSpring(BuggySpring)
{
   length = 0.42;
   force = 3400;
   damping = 1400;
   antiSwayForce = 8;
};

datablock WheeledVehicleData(BuggyData)
{
   shapeFile = "kit/buggy.dts";
   maxDamage = 1000;
   mass = 380;
   massCenter = "0 0 0.2";
   massBox = "1.6 3.8 0.9";
   drag = 0.6;
   bodyFriction = 0.6;
   bodyRestitution = 0.3;
   minImpactSpeed = 5;
   integration = 8;
   collisionTol = 0.1;
   contactTol = 0.4;
   maxSteeringAngle = 0.55;
   engineTorque = 5200;
   engineBrake = 250;
   brakeTorque = 9000;
   maxWheelSpeed = 22;
};

datablock PropData(Crate)
{
   shapeFile = "kit/cube.dts";
   size = "1 1 1";
   mass = 20;
};

// Slides for ever: no friction, no damping, never sleeps.
datablock PropData(Slider)
{
   shapeFile = "kit/cube.dts";
   size = "0.5 0.5 0.5";
   mass = 5;
   friction = 0;
   restitution = 0;
   linearDamping = 0;
   angularDamping = 0;
   sleepLinear = 0;
   sleepAngular = 0;
};

function Crate::onPropSleep(%this, %prop)
{
   $Slept++;
}

function Crate::onPropSpawn(%this, %prop)
{
   $Spawned++;
}

function startProps()
{
   new SimGroup(MissionCleanup);
   new SimGroup(ClientGroup);
   MissionCleanup.add(new StaticShape(Floor) { dataBlock = FloorData; position = "0 0 -1"; scale = "400 400 2"; });
}

function GameConnection::onConnect(%client)
{
   ClientGroup.add(%client);
   %client.transmitDataBlocks(1);
}

function GameConnection::onDataBlocksDone(%client, %sequence)
{
   %client.activateGhosting();
   if ($Test::Players)
   {
      // Three units apart, so no player stands in another's way.
      %x = $Test::Figures * 3;
      $Test::Figures++;
      %client.player = new Player() { dataBlock = FigureData; position = %x SPC "-8 0.5"; };
      MissionCleanup.add(%client.player);
      %client.setControlObject(%client.player);
      return;
   }
   %client.camera = new Camera() { dataBlock = EyeData; position = "0 -12 2"; };
   MissionCleanup.add(%client.camera);
   %client.setControlObject(%client.camera);
}
