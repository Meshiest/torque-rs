// The level demo's server. The host execs this file and then startWorld(%file) with the map the menu chose. A client that
// connects is sent the datablocks; when it has them, its ghosting starts and it is given a figure to walk (the way in and out
// is the examples' shared common/server.cs, which calls this file's hooks).

// Every client is the engine's: the engine's own classes (a map's SpotLights) are ghosted to all.
$Server::EngineClasses = 1;
// A packet to each client every tick (it sends at the slower of this and the client's rate).
$pref::Net::PacketRateToClient = 32;
// The most players the office takes when others join it (the list of matches shows it).
$Pref::Server::MaxPlayers = 8;

// ---- datablocks

datablock StaticShapeData(WorldCubeData)
{
   category = "World";
   shapeFile = "kit/cube.dts";
};

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
   // Its crouched walk and crawl play faster by the ground covered (they stride 1.2 and 0.75 a second), so no foot slides.
   crouchBoundingBox = "1 1 1.6";
   maxCrouchForwardSpeed = 1.95;
   maxCrouchBackwardSpeed = 1.65;
   maxCrouchSideSpeed = 1.5;
   crouchForce = 3600;
   proneBoundingBox = "2 2 0.8";
   maxProneForwardSpeed = 1.125;
   maxProneBackwardSpeed = 0.9;
   maxProneSideSpeed = 0;
   proneForce = 2400;

   // Swimming (the engine's own pose, after Torque3D's): in water deeper than about two thirds of the swim box's height (the
   // chest) the figure swims, its box long, at its underwater speeds. Lighter than the water, it floats with the surface 1.45
   // up its 2 m box (twice the density), where the treading figure's eye is 15 cm out (the kit's swim_root), and starts to
   // swim not far short of that depth, so the stroke is drawn at the surface from the start.
   density = 0.725;
   swimBoundingBox = "1 2.3 1.8";
   swimForce = 2400;
   maxUnderwaterForwardSpeed = 3;
   maxUnderwaterBackwardSpeed = 2;
   maxUnderwaterSideSpeed = 2;
};

// A hit's reaction over (common/server.cs).
function WorldFigureData::onEndSequence(%this, %obj, %slot)
{
   playerFlinchEnded(%obj, %slot);
}

// A figure that dies lets go of what it holds.
function WorldFigureData::onDisabled(%this, %obj, %state)
{
   dropHeld(%obj);
}

// ---- carrying props
//
// A left click asks the server (PropGrab) to pick up the prop the figure looks at, or to drop the one it holds. The server
// decides: the prop must be the first thing on the line from the figure's eye within $World::HoldReach (propRayCast, with no
// wall, shape or other figure in front of it: containerRayCast), weigh no more than $World::HoldMaxMass (a chair, a plant, a monitor, a mug; not a desk, a table, a
// sofa, a cabinet, the fridge or the copier), and have no other holder (the props plugin refuses a second). A held prop hangs
// from the figure's right hand, its arm held out (the arm sequence `hold`); a figure that dies or leaves lets go.

$World::HoldReach = 2.5;
$World::HoldMaxMass = 15;
// Where a held prop hangs with the figure looking level, in its own frame: right of its middle, its near face this far in front
// of the figure's middle (its box reaches half a unit from it), its top at the hand. The props plugin swings it about the figure's eyes with
// the view (looking up raises it, down lowers it) and keeps it clear of the box whatever the view.
$World::HoldRight = 0.3;
$World::HoldFront = 0.85;
$World::HoldHand = 1.35;

// The commands run so far: the engine holds them to the plugin's declared rate (4 a second after a burst of 8) before they
// run.
$World::GrabCommands = 0;

function serverCmdPropGrab(%client)
{
   $World::GrabCommands++;
   %player = %client.player;
   if (!isObject(%player))
      return;
   if (isObject(%player.holdJoint))
      dropHeld(%player);
   else
      grabFor(%player, %client);
}

// %player picks up the prop it looks at, for %client (0 for none) when the rules let it: the joint, or 0 with the reason in
// $World::GrabRefused.
function grabFor(%player, %client)
{
   $World::GrabRefused = "";
   if (%player.getState() $= "Dead")
      return grabRefused("dead");
   if (isObject(%player.holdJoint))
      return grabRefused("holding");
   %eye = %player.getEyePoint();
   %end = VectorAdd(%eye, VectorScale(%player.getEyeVector(), $World::HoldReach));
   %hit = propRayCast(%eye, %end);
   %prop = firstWord(%hit);
   if (%prop $= "")
      return grabRefused("nothing in reach");
   // A wall, a counter, a shape or another figure nearer on the line hides it.
   %mask = $TypeMasks::InteriorObjectType | $TypeMasks::StaticShapeObjectType | $TypeMasks::PlayerObjectType
      | $TypeMasks::TerrainObjectType;
   if (containerRayCast(%eye, getWords(%hit, 1, 3), %mask, %player))
      return grabRefused("out of sight");
   %data = %prop.dataBlock;
   if (%data.mass > $World::HoldMaxMass)
      return grabRefused("too heavy");
   %size = %data.size;
   %offset = $World::HoldRight SPC $World::HoldFront + getWord(%size, 1) / 2
      SPC getMax($World::HoldHand - getWord(%size, 2) / 2, getWord(%size, 2) / 2 + 0.05);
   %joint = jointHold(%player, %prop, %offset, %client);
   if (!%joint)
      return grabRefused("held");
   %player.holdJoint = %joint;
   %joint.holder = %player;
   %player.setArmThread("hold");
   return %joint;
}

function grabRefused(%why)
{
   $World::GrabRefused = %why;
   return 0;
}

// A hold the props plugin broke (its prop frozen, or stuck behind a wall): its holder lets go.
function Joint::onBreak(%this)
{
   if (isObject(%this.holder) && %this.holder.holdJoint == %this.getId())
      dropHeld(%this.holder);
}

// %player lets go of what it holds: the prop falls where it is, the arm hangs again.
function dropHeld(%player)
{
   if (isObject(%player.holdJoint))
      jointDelete(%player.holdJoint);
   %player.holdJoint = "";
   if (%player.getState() !$= "Dead")
      %player.setArmThread("look");
}

// ---- the world

exec("common/server.cs");

// Loads the map %file (a script that builds MissionGroup and a MapInfo object with the title, the spawn point and the yaw).
function startWorld(%file)
{
   loadMission(%file);
   if (!isObject(MapInfo))
      new ScriptObject(MapInfo) { title = fileBase(%file); spawn = "0 0 1"; spawnYaw = 0; };
   $World::Title = MapInfo.title;
   echo("World" SPC $World::Title SPC "with" SPC MissionGroup.getCount() SPC "objects");
   schedule(500, 0, "watchFalls");
}

// ---- a client's way in

// The name a client connected with ($Pref::Player::Name, when it has one) is the name its figure shows and a game's scripts list.
function GameConnection::onClientConnect(%client, %name)
{
   if (%name !$= "")
      %client.name = cleanName(%name, %client);
   echo("Client" SPC %client SPC "connected");
}

// Players start side by side, 1.5 apart along the map's x in the order they joined, so nobody starts inside another (the first
// at the map's spawn point).
function GameConnection::spawnFigure(%client)
{
   %place = 0;
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
      if (ClientGroup.getObject(%i) == %client)
         %place = %i;
   %spawn = getWord(MapInfo.spawn, 0) + (%place % 8) * 1.5 SPC getWords(MapInfo.spawn, 1, 2);
   $World::Figure = %client.createFigure(WorldFigureData, %spawn SPC "0 0 1" SPC MapInfo.spawnYaw);
}

function GameConnection::onClientEnterGame(%client)
{
   commandToClient(%client, 'WorldStarted', $World::Title);
}

function GameConnection::onClientLeaveGame(%client)
{
   // Its figure is gone already, and the props plugin let go of what it held with it.
}

// The client asked to start again at the spawn point.
function serverCmdRespawn(%client)
{
   %client.spawnFigure();
   commandToClient(%client, 'Respawned');
}

// A figure that fell off the map starts again.
function GameConnection::onFellOff(%client)
{
   %client.spawnFigure();
   commandToClient(%client, 'Fell');
}
