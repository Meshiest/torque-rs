// The trace-hash server: a flat floor, a figure to walk and a buggy to drive. A connection controls the figure, which stays on
// foot or sits at the buggy's wheel and drives it, as `$Trace::Control` says ("figure" or "buggy"); the harness sets it before
// the run.

datablock PlayerData(TraceFigureData)
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
   jumpDelay = 10;
   minJumpSpeed = 15;
   maxJumpSpeed = 30;
   jumpSurfaceAngle = 70;
   minImpactSpeed = 40;
   maxLookAngle = 1.4;
   minLookAngle = -1.4;
   crouchBoundingBox = "1 1 1.6";
   maxCrouchForwardSpeed = 1.95;
   maxCrouchBackwardSpeed = 1.65;
   maxCrouchSideSpeed = 1.5;
   crouchForce = 3600;
};

datablock StaticShapeData(TraceFloorData)
{
   shapeFile = "kit/cube.dts";
};

datablock WheeledVehicleTire(TraceTire)
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

datablock WheeledVehicleSpring(TraceSpring)
{
   length = 0.42;
   force = 3400;
   damping = 1400;
   antiSwayForce = 8;
};

datablock WheeledVehicleData(TraceBuggyData)
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

function TraceBuggyData::onAdd(%this, %obj)
{
   for (%i = 0; %i < %obj.getWheelCount(); %i++)
   {
      %obj.setWheelTire(%i, TraceTire);
      %obj.setWheelSpring(%i, TraceSpring);
      %obj.setWheelPowered(%i, true);
   }
   %obj.setWheelSteering(0, 1);
   %obj.setWheelSteering(1, 1);
}

// The world: a floor 4000 units square with its top at z = 0, the figure at the origin and the buggy beside it.
function startTrace()
{
   new SimGroup(MissionCleanup);
   %floor = new StaticShape(TraceFloor)
   {
      dataBlock = TraceFloorData;
      position = "0 0 -1";
      rotation = "1 0 0 0";
      scale = "4000 4000 2";
   };
   MissionCleanup.add(%floor);
   %figure = new Player(TraceFigure)
   {
      dataBlock = TraceFigureData;
      position = "0 0 0.5";
      rotation = "1 0 0 0";
   };
   MissionCleanup.add(%figure);
   %buggy = new WheeledVehicle(TraceBuggy)
   {
      dataBlock = TraceBuggyData;
      position = "8 0 1.5";
      rotation = "1 0 0 0";
   };
   MissionCleanup.add(%buggy);
}

function GameConnection::onConnect(%client)
{
   %client.transmitDataBlocks(1);
}

function GameConnection::onDataBlocksDone(%client, %sequence)
{
   %client.activateGhosting();
   // A later connection a scenario gives an object of its own to control.
   if (isObject($Trace::Next))
   {
      %client.setControlObject($Trace::Next);
      $Trace::Next = "";
      return;
   }
   // The connection controls the figure; for the buggy the figure sits at its wheel and its control object is the buggy.
   if ($Trace::Control $= "buggy")
   {
      TraceBuggy.mountObject(TraceFigure, 0);
      TraceFigure.setTransform("0 0 0 0 0 1 0");
      TraceFigure.setControlObject(TraceBuggy);
   }
   %client.setControlObject(TraceFigure);
}
