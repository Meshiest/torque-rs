// The vehicle demo's server, for one player or a few on the same network. The host execs this file and then startWorld(%file)
// with the map. A client that connects is sent the datablocks; when it has them, its ghosting starts and it is given a figure
// beside the buggy (the way in and out is the examples' shared common/server.cs, which calls this file's hooks).
//
// Getting in: the client asks (serverCmdGetIn) when its player clicks; a ray from the figure's eye along its look, $World::Reach
// long, that meets the buggy seats the figure in the free seat nearest the point it met: the driver's (node 0, on the left),
// whose figure controls the buggy (the client's moves drive it), or the passenger's (node 1, on the right), who rides. Getting
// out (serverCmdGetOut) puts the figure on its feet beside its seat's side.
//
// The headlight: each buggy carries a SpotLight on its `mount2` node (between the lamps on its roof), off until its driver
// presses F (serverCmdToggleLight); while it is on, the buggy plays `lights_on` (the roof lamps' faces lit). The day
// (TimeOfDay, ScatterSky) and the headlight are the engine's own classes: every client of the demo is the engine's
// ($Server::EngineClasses).
//
// A buggy for each player: the map's (Buggy) is the first player's; each later player is given one of its own as it enters
// (giveBuggy), in a row along +x, $World::BuggyApart apart, beside the player's spawn point, at most $World::MaxBuggies in all.
// Anyone may take any free seat of any buggy. A player's buggy goes when the player leaves, at once when nobody sits in it,
// else once its last rider gets out (dropIfEmpty).
//
// The brake: the demo's Player hands a driver's jump trigger to the buggy (its brake), and no key of a move dismounts
// (src/seats.rs); off the throttle the buggy coasts.
//
// Starting again: R (serverCmdRespawn) puts the player on foot beside its own buggy, and the buggy back in its place (upright,
// stopped, its headlight off), anyone in it out and standing beside it first. A buggy that falls off the map (it, or a rider
// in it, below $Server::FallHeight) goes back to its place the same way, its riders out and beside it (resetBuggy).
//
// The day's pace: the host (the first client in) may run the day faster or slower ($World::DaySpeeds, 0 stopping it) or back
// at its own pace (serverCmdDaySpeed); the others are told only the host may.
//
// Credit: the buggy's datablocks take the fields of TGE's WheeledVehicleData, WheeledVehicleTire and WheeledVehicleSpring, and
// the way in and out follows the mount and control calls of Torque3D's vehicle scripts (MIT: `scripts/server/player.cs` and
// `vehicle.cs` of its templates, Copyright (c) 2012 GarageGames, LLC). The values and the code here are the example's own.

// ---- datablocks

datablock PlayerData(WorldFigureData)
{
   category = "World";
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

   // Crouched and prone (the engine's own poses, after Torque3D's: its fields, by their names): each pose's box and speeds.
   // The kit's figure has the poses' sequences (crouch_root, prone_root and the rest).
   crouchBoundingBox = "1 1 1.6";
   maxCrouchForwardSpeed = 1.3;
   maxCrouchBackwardSpeed = 1.1;
   maxCrouchSideSpeed = 1;
   crouchForce = 3600;
   proneBoundingBox = "2 2 0.8";
   maxProneForwardSpeed = 0.75;
   maxProneBackwardSpeed = 0.6;
   maxProneSideSpeed = 0;
   proneForce = 2400;
};

// A hit's reaction over (common/server.cs).
function WorldFigureData::onEndSequence(%this, %obj, %slot)
{
   playerFlinchEnded(%obj, %slot);
}

// The dust the tires kick up on the grass: soft, short-lived puffs of dry earth and torn grass, a little drift up, no wall of
// smoke (the engine runs the tire emitter as TGE does: more the faster the buggy goes and the more its tires slip).
datablock ParticleData(BuggyDustPuff)
{
   textureName = "kit/smoke";
   dragCoefficient = 2.5;
   gravityCoefficient = -0.04;
   inheritedVelFactor = 0.8;
   lifetimeMS = 900;
   lifetimeVarianceMS = 300;
   useInvAlpha = true;
   spinRandomMin = -60;
   spinRandomMax = 60;

   colors[0] = "0.64 0.57 0.38 0.0";
   colors[1] = "0.6 0.56 0.38 0.6";
   colors[2] = "0.54 0.57 0.4 0.0";
   sizes[0] = 0.6;
   sizes[1] = 1.4;
   sizes[2] = 2.2;
   times[0] = 0;
   times[1] = 0.25;
   times[2] = 1;
};

datablock ParticleEmitterData(BuggyDust)
{
   ejectionPeriodMS = 4;
   periodVarianceMS = 1;
   ejectionVelocity = 1.4;
   velocityVariance = 0.6;
   ejectionOffset = 0.1;
   thetaMin = 70;
   thetaMax = 110;
   phiReferenceVel = 0;
   phiVariance = 360;
   particles = "BuggyDustPuff";
};

datablock WheeledVehicleTire(BuggyTire)
{
   shapeFile = "kit/buggy_wheel.dts";
   // The tire's grip: how hard it holds the ground at rest and sliding, and its springiness along and across its roll.
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
   // Long and soft enough to show over the hills; the anti-sway force keeps it flat in a turn.
   length = 0.42;
   force = 3400;
   damping = 1400;
   antiSwayForce = 8;
};

datablock WheeledVehicleData(BuggyData)
{
   category = "Vehicles";
   shapeFile = "kit/buggy.dts";

   maxDamage = 1000;
   mass = 380;
   // Low and in the middle, the box it turns about as long and wide as the frame: not tippy.
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
   // Off the throttle the wheels slow gently (it coasts); the brake (Space) holds them hard.
   engineBrake = 250;
   brakeTorque = 9000;
   maxWheelSpeed = 22;

   cameraMaxDist = 7.5;
   cameraOffset = 1.5;
   tireEmitter = BuggyDust;
};

// Every wheel takes the tire and the spring, all four are driven, the front pair steers.
function BuggyData::onAdd(%this, %obj)
{
   for (%i = 0; %i < %obj.getWheelCount(); %i++)
   {
      %obj.setWheelTire(%i, BuggyTire);
      %obj.setWheelSpring(%i, BuggySpring);
      %obj.setWheelPowered(%i, true);
   }
   %obj.setWheelSteering(0, 1);
   %obj.setWheelSteering(1, 1);
}

// The seats: where a seated figure's hips are in the buggy's space (its mount node and the hips above it), and the side a
// figure gets out on (-1 left, 1 right).
$Buggy::SeatCount = 2;
$Buggy::Seat[0] = "-0.32 -0.3 0.66";
$Buggy::Seat[1] = "0.32 -0.3 0.66";
$Buggy::Side[0] = -1;
$Buggy::Side[1] = 1;

// ---- the world

exec("common/server.cs");

// Every client is the engine's: the engine's own classes (the headlight, the day) are ghosted to all.
$Server::EngineClasses = 1;

// How far a click reaches from the figure's eye, and what it looks for (VehicleObjectType).
$World::Reach = 5.5;
$World::MaxBuggies = 16;
$World::BuggyApart = 8;
$World::VehicleMask = 65536;
// TerrainObjectType: what the ground is.
$World::TerrainMask = 4;

// The ground's height at %x %y (0 off the terrain).
function groundAt(%x, %y)
{
   %hit = containerRayCast(%x SPC %y SPC 1000, %x SPC %y SPC -100, $World::TerrainMask);
   if (%hit $= "")
      return 0;
   return getWord(%hit, 3);
}

// Loads the map %file (a script that builds MissionGroup and a MapInfo object with the title and the spawn points).
function startWorld(%file)
{
   loadMission(%file);
   $World::Title = MapInfo.title;
   $World::Buggy = Buggy.getId();
   $World::Buggies = 1;
   Buggy.home = Buggy.getTransform();
   giveHeadlight(Buggy);
   $World::DayLength = WorldDay.dayLength;
   $World::DaySpeed = 1;
   echo("World" SPC $World::Title SPC "with" SPC MissionGroup.getCount() SPC "objects");
   schedule(500, 0, "watchFalls");
   schedule(500, 0, "watchBuggyFalls");
}

// A headlight between the lamps on %vehicle's roof (its mount2 node), off.
function giveHeadlight(%vehicle)
{
   %lamp = new SpotLight()
   {
      range = 50;
      innerAngle = 24;
      outerAngle = 52;
      brightness = 0.85;
      color = "1 0.98 0.94";
      castShadows = 1;
   };
   MissionCleanup.add(%lamp);
   %vehicle.mountObject(%lamp, 2);
   %lamp.setLightEnabled(false);
   %vehicle.headlight = %lamp;
}

// ---- a client's way in

function GameConnection::onClientConnect(%client, %name)
{
   echo("Client" SPC %client SPC "connected");
   // The host: the first client in (the one that started the game).
   if (!isObject($World::Host))
      $World::Host = %client;
}

// The client's place in ClientGroup (the order the clients came in).
function GameConnection::joinOrder(%client)
{
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
      if (ClientGroup.getObject(%i) == %client)
         return %i;
   return 0;
}

// A figure at the client's spawn point: the first client's on the map's buggy's left, the second's on its right, each later
// one's on the left of its own buggy's place.
function GameConnection::spawnFigure(%client)
{
   %k = %client.joinOrder();
   if (%k < 2)
   {
      %x = getWord(MapInfo.spawn[%k], 0);
      %y = getWord(MapInfo.spawn[%k], 1);
      %yaw = MapInfo.spawnYaw[%k];
   }
   else
   {
      %x = %k * $World::BuggyApart + getWord(MapInfo.spawn0, 0);
      %y = getWord(MapInfo.spawn0, 1);
      %yaw = MapInfo.spawnYaw0;
   }
   %at = %x SPC %y SPC groundAt(%x, %y) + 0.1;
   %client.createFigure(WorldFigureData, %at SPC "0 0 1" SPC %yaw);
}

function GameConnection::onClientEnterGame(%client)
{
   commandToClient(%client, 'WorldStarted', $World::Title);
   giveBuggy(%client);
}

// The client's own buggy: the map's for the first client, a new one in its place along the row for each later one (none past
// $World::MaxBuggies).
function giveBuggy(%client)
{
   if (isObject(Buggy) && !isObject(Buggy.owner))
   {
      Buggy.owner = %client;
      Buggy.ownerLeft = 0;
      %client.buggy = Buggy;
      return;
   }
   if ($World::Buggies >= $World::MaxBuggies)
      return;
   %k = freeBuggySlot();
   %x = %k * $World::BuggyApart;
   %vehicle = new WheeledVehicle()
   {
      dataBlock = BuggyData;
      position = %x SPC "0" SPC groundAt(%x, 0) + 1.2;
      rotation = "0 0 1 0";
   };
   MissionCleanup.add(%vehicle);
   %vehicle.home = %vehicle.getTransform();
   giveHeadlight(%vehicle);
   %vehicle.owner = %client;
   %vehicle.slot = %k;
   %client.buggy = %vehicle;
   $World::Buggies++;
}

// The lowest place in the row (1 up; the map's buggy stands at 0) no spawned buggy holds, so a newcomer's buggy never lands
// on one left standing by a player who went.
function freeBuggySlot()
{
   for (%k = 1; %k <= $World::MaxBuggies; %k++)
   {
      %taken = false;
      for (%i = MissionCleanup.getCount() - 1; %i >= 0 && !%taken; %i--)
      {
         %obj = MissionCleanup.getObject(%i);
         if (%obj.getClassName() $= "WheeledVehicle" && %obj.slot == %k)
            %taken = true;
      }
      if (!%taken)
         return %k;
   }
   return $World::MaxBuggies;
}

// The client left (its figure already gone): its buggy is let go, and every let-go buggy nobody sits in now goes (the figure
// may have been riding in one).
function GameConnection::onClientLeaveGame(%client)
{
   if (isObject(%client.buggy))
   {
      %client.buggy.owner = 0;
      %client.buggy.ownerLeft = 1;
   }
   for (%i = MissionCleanup.getCount() - 1; %i >= 0; %i--)
   {
      %obj = MissionCleanup.getObject(%i);
      if (%obj.getClassName() $= "WheeledVehicle")
         dropIfEmpty(%obj);
   }
   dropIfEmpty(Buggy);
}

// Deletes a buggy whose owner left once nobody sits in it (its headlight with it).
function dropIfEmpty(%vehicle)
{
   if (!isObject(%vehicle) || !%vehicle.ownerLeft)
      return;
   for (%i = 0; %i < $Buggy::SeatCount; %i++)
   {
      %rider = %vehicle.getMountNodeObject(%i);
      if (isObject(%rider) && %rider.getClassName() $= "Player" && %rider.client.player == %rider)
         return;
   }
   if (isObject(%vehicle.headlight))
      %vehicle.headlight.delete();
   %vehicle.delete();
   $World::Buggies--;
}

// The client asked to start again: out of any seat, its own buggy back in its place (resetBuggy), and the figure on foot
// beside the buggy's left (at the spawn point without one).
function serverCmdRespawn(%client)
{
   %player = %client.player;
   if (isObject(%player) && %player.isMounted())
   {
      %vehicle = %player.getObjectMount();
      getOff(%player);
      dropIfEmpty(%vehicle);
   }
   %client.spawnFigure();
   if (isObject(%client.buggy))
   {
      resetBuggy(%client.buggy);
      standBeside(%client.player, %client.buggy, -1);
   }
   commandToClient(%client, 'Respawned');
}

// A figure that fell off the map starts again; one in a seat goes back with the buggy it sits in.
function GameConnection::onFellOff(%client)
{
   %player = %client.player;
   if (isObject(%player) && %player.isMounted())
   {
      buggyFell(%player.getObjectMount());
      return;
   }
   %client.spawnFigure();
   commandToClient(%client, 'Fell');
}

// Every half second: a buggy below $Server::FallHeight goes back to its place with its riders.
function watchBuggyFalls()
{
   for (%i = MissionCleanup.getCount() - 1; %i >= 0; %i--)
   {
      %obj = MissionCleanup.getObject(%i);
      if (%obj.getClassName() $= "WheeledVehicle" && getWord(%obj.getTransform(), 2) < $Server::FallHeight)
         buggyFell(%obj);
   }
   if (isObject(Buggy) && getWord(Buggy.getTransform(), 2) < $Server::FallHeight)
      buggyFell(Buggy);
   schedule(500, 0, "watchBuggyFalls");
}

// %vehicle fell off the map: it goes back to its place, each rider told it fell.
function buggyFell(%vehicle)
{
   for (%i = 0; %i < $Buggy::SeatCount; %i++)
   {
      %rider = %vehicle.getMountNodeObject(%i);
      if (isObject(%rider) && isObject(%rider.client))
         commandToClient(%rider.client, 'Fell');
   }
   resetBuggy(%vehicle);
}

// %vehicle back in its place (its home: where the map or giveBuggy put it), upright, stopped, its headlight off; anyone in
// it gets out first and stands beside it on the side of the seat it had.
function resetBuggy(%vehicle)
{
   %n = 0;
   for (%i = 0; %i < $Buggy::SeatCount; %i++)
   {
      %rider = %vehicle.getMountNodeObject(%i);
      if (isObject(%rider) && %rider.getClassName() $= "Player")
      {
         getOff(%rider);
         %out[%n] = %rider;
         %side[%n] = $Buggy::Side[%i];
         %n++;
      }
   }
   %vehicle.setTransform(%vehicle.home);
   stopVehicle(%vehicle);
   if (isObject(%vehicle.headlight))
      setHeadlight(%vehicle, false);
   for (%k = 0; %k < %n; %k++)
      standBeside(%out[%k], %vehicle, %side[%k]);
}

// ---- in and out of the buggy

// The client clicked: the free seat of the vehicle the figure looks at within reach, the one nearest the point it looked at.
function serverCmdGetIn(%client)
{
   %player = %client.player;
   if (!isObject(%player) || %player.isMounted())
      return;
   %eye = %player.getEyePoint();
   %end = VectorAdd(%eye, VectorScale(%player.getEyeVector(), $World::Reach));
   %hit = containerRayCast(%eye, %end, $World::VehicleMask, %player);
   %vehicle = getWord(%hit, 0);
   if (!isObject(%vehicle))
      return;
   %at = getWords(%hit, 1, 3);
   %xf = %vehicle.getTransform();
   %seat = -1;
   for (%i = 0; %i < $Buggy::SeatCount; %i++)
   {
      if (isObject(%vehicle.getMountNodeObject(%i)))
         continue;
      %d = VectorLen(VectorSub(MatrixMulPoint(%xf, $Buggy::Seat[%i]), %at));
      if (%seat < 0 || %d < %best)
      {
         %seat = %i;
         %best = %d;
      }
   }
   if (%seat < 0)
      return;
   %vehicle.mountObject(%player, %seat);
   // Seated facing the way the seat faces: a mounted object's transform is its place on the mount.
   %player.setTransform("0 0 0 0 0 1 0");
   %player.setActionThread(%seat == 0 ? "sit" : "ride", true, true);
   // The driver controls the vehicle; the passenger rides.
   if (%seat == 0)
   {
      straightenWheels(%vehicle);
      %player.setControlObject(%vehicle);
   }
   commandToClient(%client, 'Seated', %seat);
}

// The client asked to switch its vehicle's headlight: only the driver may.
function serverCmdToggleLight(%client)
{
   %player = %client.player;
   if (!isObject(%player) || !%player.isMounted())
      return;
   %vehicle = %player.getObjectMount();
   if (%vehicle.getMountNodeObject(0) != %player.getId() || !isObject(%vehicle.headlight))
      return;
   setHeadlight(%vehicle, !%vehicle.headlight.isEnabled);
}

// Switches %vehicle's headlight and its roof lamps' glow (thread 0) on or off.
function setHeadlight(%vehicle, %on)
{
   %vehicle.headlight.setLightEnabled(%on);
   if (%on)
      %vehicle.playThread(0, "lights_on");
   else
      %vehicle.stopThread(0);
}

// The client asked to get out: the figure stands beside its seat's side of the vehicle, facing the way it faces.
function serverCmdGetOut(%client)
{
   %player = %client.player;
   if (!isObject(%player) || !%player.isMounted())
      return;
   %vehicle = %player.getObjectMount();
   %side = 0;
   for (%i = 0; %i < $Buggy::SeatCount; %i++)
      if (%vehicle.getMountNodeObject(%i) == %player.getId())
         %side = $Buggy::Side[%i];
   getOff(%player);
   standBeside(%player, %vehicle, %side);
   dropIfEmpty(%vehicle);
}

// %player out of its seat, controlling only itself, standing, its client told it is on foot.
function getOff(%player)
{
   %player.unmount();
   %player.setControlObject(0);
   %player.setActionThread("root");
   if (isObject(%player.client))
      commandToClient(%player.client, 'Seated', -1);
}

// %player on its feet beside %vehicle on its %side (-1 left, 1 right), facing the way the vehicle faces, still.
function standBeside(%player, %vehicle, %side)
{
   %xf = %vehicle.getTransform();
   %beside = VectorAdd(getWords(%xf, 0, 2), MatrixMulVector(%xf, 2.4 * %side SPC "0 0"));
   %x = getWord(%beside, 0);
   %y = getWord(%beside, 1);
   %z = getMax(groundAt(%x, %y), getWord(%beside, 2) - 1) + 0.2;
   %forward = MatrixMulVector(%xf, "0 1 0");
   %yaw = mAtan(getWord(%forward, 0), getWord(%forward, 1));
   %player.setTransform(%x SPC %y SPC %z SPC "0 0 1" SPC %yaw);
   %player.setVelocity("0 0 0");
}

// ---- the day's pace

// The paces the day runs at, its own pace times each (0 stops it); ] and [ step along them.
$World::DaySpeeds = "0 1 4 16 64 256";

// The client asked for the day faster (%step 1), slower (-1) or at its own pace (0): only the host may.
function serverCmdDaySpeed(%client, %step)
{
   if (%client != $World::Host || !isObject(WorldDay))
   {
      commandToClient(%client, 'DaySpeed', "", "", "Only the host can change the time");
      return;
   }
   %count = getWordCount($World::DaySpeeds);
   %at = 1;
   for (%i = 0; %i < %count; %i++)
      if (getWord($World::DaySpeeds, %i) == $World::DaySpeed)
         %at = %i;
   if (%step == 0)
      %at = 1;
   else
      %at = getMax(0, getMin(%count - 1, %at + %step));
   $World::DaySpeed = getWord($World::DaySpeeds, %at);
   if ($World::DaySpeed == 0)
      WorldDay.setPlay(false);
   else
   {
      WorldDay.setDayLength($World::DayLength / $World::DaySpeed);
      WorldDay.setPlay(true);
   }
   commandToClient(%client, 'DaySpeed', $World::DaySpeed, WorldDay.time);
}
