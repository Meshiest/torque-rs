// The arena's server. The host sets $Pref::Server::Name, execs this file and calls startArena(%file) with the first map to
// play. A round on a map lasts $Pref::Server::RoundSeconds (five minutes) or until a player's score reaches
// $Pref::Server::ScoreLimit; then every client sees the round's scores for a few seconds and the next map loads (the
// rotation, at the end of this file). The host may also pick the map from the pause menu.
// A client that connects is sent the datablocks; when it has them, its ghosting starts and it gets a figure holding a pistol
// with an 11-round magazine, reloaded as often as it likes (R, or by itself when empty). Bolts hurt figures; three hits knock
// one out: it falls over, away from the shot, and comes back at a spawn point a moment later. The way in and out is the
// examples' shared common/server.cs, which calls this file's hooks.

// ---- datablocks

$Arena::ReloadSeconds = 1.2;
// The figure, the pistol and the cube are drawn from the kit's .dts files, or from its .glb exports when the host sets
// $Arena::Shapes to "glb" (the same shapes either way).
if ($Arena::Shapes !$= "glb")
   $Arena::Shapes = "dts";

datablock StaticShapeData(ArenaCube)
{
   category = "Arena";
   shapeFile = "kit/cube." @ $Arena::Shapes;
};

datablock PlayerData(ArenaFigure)
{
   category = "Arena";
   shapeFile = "kit/figure." @ $Arena::Shapes;

   mass = 90;
   drag = 0.3;
   maxDamage = 100;
   boundingBox = "1 1 2";
   // Bolts hit the figure's parts as it stands (its shape's LOS boxes), not the box; the box still moves it.
   hitboxes = true;

   runForce = 3600;
   maxForwardSpeed = 7;
   maxBackwardSpeed = 5;
   maxSideSpeed = 6;
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
};

// The bolt's trail: small bright sparks added over what is behind them, fading from white through a pale cyan to nothing as
// they shrink. A particle's life goes to the client in steps of 32 ms.
datablock ParticleData(ArenaBoltSpark)
{
   textureName = "kit/spark";
   dragCoefficient = 2;
   gravityCoefficient = 0;
   inheritedVelFactor = 0;
   lifetimeMS = 160;
   lifetimeVarianceMS = 32;
   useInvAlpha = false;
   spinRandomMin = -90;
   spinRandomMax = 90;

   colors[0] = "1 1 1 0.85";
   colors[1] = "0.7 0.92 1 0.5";
   colors[2] = "0.45 0.75 1 0";
   sizes[0] = 0.09;
   sizes[1] = 0.06;
   sizes[2] = 0.02;
   times[0] = 0;
   times[1] = 0.35;
   times[2] = 1;
};

datablock ParticleEmitterData(ArenaBoltTrail)
{
   ejectionPeriodMS = 1;
   periodVarianceMS = 0;
   ejectionVelocity = 0.6;
   velocityVariance = 0.3;
   thetaMin = 0;
   thetaMax = 20;
   phiReferenceVel = 0;
   phiVariance = 360;
   particles = "ArenaBoltSpark";
};

// A bolt hitting a figure bursts into hot red sparks (the client bursts them where a bolt hits a figure only).
datablock ParticleData(ArenaHitSpark)
{
   textureName = "kit/spark";
   dragCoefficient = 4;
   gravityCoefficient = 0.6;
   inheritedVelFactor = 0;
   lifetimeMS = 320;
   lifetimeVarianceMS = 96;
   useInvAlpha = false;
   spinRandomMin = -180;
   spinRandomMax = 180;

   colors[0] = "1 0.95 0.8 1";
   colors[1] = "1 0.3 0.15 0.8";
   colors[2] = "0.6 0.05 0.02 0";
   sizes[0] = 0.18;
   sizes[1] = 0.12;
   sizes[2] = 0.04;
   times[0] = 0;
   times[1] = 0.3;
   times[2] = 1;
};

datablock ParticleEmitterData(ArenaHitBurst)
{
   ejectionPeriodMS = 1;
   periodVarianceMS = 0;
   ejectionVelocity = 5;
   velocityVariance = 2;
   thetaMin = 0;
   thetaMax = 180;
   phiReferenceVel = 0;
   phiVariance = 360;
   particles = "ArenaHitSpark";
};

datablock ExplosionData(ArenaBoltHit)
{
   particleEmitter = ArenaHitBurst;
   particleDensity = 36;
   particleRadius = 0.15;
   lifetimeMS = 320;
};

// Where a bolt hits the world it leaves a small cracked hole, one of three (the client lays it on the face it hit, and fades
// it out after a while).
datablock DecalData(ArenaBoltHole)
{
   sizeX = 0.24;
   sizeY = 0.24;
   textureName = "kit/hole";
};

datablock DecalData(ArenaBoltHole2)
{
   sizeX = 0.21;
   sizeY = 0.21;
   textureName = "kit/hole2";
};

datablock DecalData(ArenaBoltHole3)
{
   sizeX = 0.27;
   sizeY = 0.27;
   textureName = "kit/hole3";
};

datablock ProjectileData(ArenaBolt)
{
   explosion = ArenaBoltHit;
   decals[0] = ArenaBoltHole;
   decals[1] = ArenaBoltHole2;
   decals[2] = ArenaBoltHole3;
   projectileShapeName = "kit/projectile.dts";
   particleEmitter = ArenaBoltTrail;
   directDamage = 34;
   muzzleVelocity = 150;
   velInheritFactor = 0;
   armingDelay = 0;
   lifetime = 1000;
   fadeDelay = 900;
   isBallistic = false;
};

// After a shot, a little smoke drifts out of the muzzle and follows it as the pistol moves: soft gray puffs blended over what
// is behind them, swelling and fading as they rise. (The flash itself is the client's: a star at the muzzle.)
datablock ParticleData(ArenaMuzzleSmokePuff)
{
   textureName = "kit/smoke";
   dragCoefficient = 3;
   gravityCoefficient = -0.06;
   inheritedVelFactor = 0;
   lifetimeMS = 640;
   lifetimeVarianceMS = 192;
   useInvAlpha = true;
   spinRandomMin = -60;
   spinRandomMax = 60;

   colors[0] = "0.85 0.85 0.86 0.42";
   colors[1] = "0.8 0.8 0.82 0.25";
   colors[2] = "0.75 0.75 0.78 0";
   sizes[0] = 0.04;
   sizes[1] = 0.12;
   sizes[2] = 0.22;
   times[0] = 0;
   times[1] = 0.4;
   times[2] = 1;
};

datablock ParticleEmitterData(ArenaMuzzleSmoke)
{
   ejectionPeriodMS = 40;
   periodVarianceMS = 10;
   ejectionVelocity = 0.25;
   velocityVariance = 0.1;
   thetaMin = 0;
   thetaMax = 20;
   phiReferenceVel = 0;
   phiVariance = 360;
   particles = "ArenaMuzzleSmokePuff";
};

datablock ShapeBaseImageData(ArenaPistolImage)
{
   shapeFile = "kit/pistol." @ $Arena::Shapes;
   mountPoint = 0;
   eyeOffset = "0.26 0.5 -0.24";
   correctMuzzleVector = true;
   firstPerson = true;
   className = "ArenaWeapon";
   projectile = ArenaBolt;
   projectileType = Projectile;

   stateName[0] = "Activate";
   stateTimeoutValue[0] = 0.25;
   stateTransitionOnTimeout[0] = "Ready";

   // The magazine empty (or a reload asked for: the script clears the image's ammo) reloads it.
   stateName[1] = "Ready";
   stateTransitionOnNoAmmo[1] = "Reload";
   stateTransitionOnTriggerDown[1] = "Fire";

   stateName[2] = "Fire";
   stateFire[2] = true;
   stateAllowImageChange[2] = false;
   stateSequence[2] = "fire";
   stateDirection[2] = true;
   stateRecoil[2] = LightRecoil;
   stateScript[2] = "onFire";
   stateEmitter[2] = ArenaMuzzleSmoke;
   stateEmitterTime[2] = 0.7;
   stateEmitterNode[2] = "muzzlePoint";
   stateTimeoutValue[2] = 0.15;
   stateTransitionOnTimeout[2] = "Release";

   // One round a click: the trigger has to come up before the next. The last round spent reloads at once.
   stateName[3] = "Release";
   stateTransitionOnNoAmmo[3] = "Reload";
   stateTransitionOnTriggerUp[3] = "Ready";

   // Reloading: a full magazine when the time runs out (there is no count of spare rounds).
   stateName[4] = "Reload";
   stateAllowImageChange[4] = false;
   stateScript[4] = "onReload";
   stateTimeoutValue[4] = $Arena::ReloadSeconds;
   stateTransitionOnTimeout[4] = "Reloaded";

   stateName[5] = "Reloaded";
   stateScript[5] = "onReloaded";
   stateTransitionOnAmmo[5] = "Ready";
};

// ---- the arena

exec("common/server.cs");

$Arena::Rounds = 11;
$Arena::RespawnMs = 2500;

// A packet to each client every tick (TGE's most; its default is ten a second): the other figures move on smaller steps.
$pref::Net::PacketRateToClient = 32;

$Server::GameType = "Arena";
$Server::MissionType = "Free for all";
$Server::PlayerCount = 0;
$Pref::Server::MaxPlayers = 16;
// A bolt hits the figures where its shooter saw them: as they were one round trip ago (up to the default cap).
$Pref::Server::LagCompensation = 1;
if ($Pref::Server::Name $= "")
   $Pref::Server::Name = "Arena";

// Loads the map %file: a script that builds MissionGroup and sets $Arena::SpawnCount and $Arena::Spawn[i] ("x y z yaw").
// MissionGroup's textureFilter field ("nearest", or none) is how the clients draw the map's textures. The clients already
// in the game download it in place of the last one, and a new round starts.
function startArena(%file)
{
   $Arena::SpawnCount = 0;
   $Arena::NextSpawn = 0;
   $Arena::Title = fileBase(%file);
   loadMission(%file);
   if ($Arena::SpawnCount == 0)
   {
      $Arena::Spawn[0] = "0 0 1 0";
      $Arena::SpawnCount = 1;
   }
   $Server::MissionName = $Arena::Title;
   echo("Arena" SPC $Arena::Title SPC "with" SPC MissionGroup.getCount() SPC "objects and" SPC $Arena::SpawnCount SPC "spawn points");
   schedule(500, 0, "watchFalls");
   startRound();
}

// The LAN list's line about this game.
function onServerInfoQuery()
{
   return "In the arena";
}

// ---- a client's way in
//
// The name cleaned (cleanName), the lines to every client (messageAll) and the players' chat (serverCmdSay, with its limits)
// are common/server.cs's.

function GameConnection::onClientConnect(%client, %name)
{
   %client.name = cleanName(%name, %client);
   %client.score = 0;
   %client.entered = false;
   $Server::PlayerCount++;
   echo("Client" SPC %client SPC "joined as" SPC %client.name);
}

// Called on each map the client enters: the first time it joined the game, after that the map changed under it.
function GameConnection::onClientEnterGame(%client)
{
   commandToClient(%client, 'ArenaStarted', $Arena::Title, $Pref::Server::Name, MissionGroup.textureFilter);
   commandToClient(%client, 'RoundTime', roundSecondsLeft());
   if (%client.isHost)
      sendMapChoices(%client);
   if (!%client.entered)
      messageAll("info", %client.name SPC "joined the arena.");
   %client.entered = true;
}

function GameConnection::onClientLeaveGame(%client)
{
   $Server::PlayerCount--;
   messageAll("info", %client.name SPC "left the arena.");
}

// The next spawn point, around the map's list.
function nextSpawn()
{
   %spawn = $Arena::Spawn[$Arena::NextSpawn % $Arena::SpawnCount];
   $Arena::NextSpawn++;
   return %spawn;
}

function GameConnection::spawnFigure(%client)
{
   cancel(%client.respawn);
   %spawn = nextSpawn();
   %player = %client.createFigure(ArenaFigure, getWords(%spawn, 0, 2) SPC "0 0 1" SPC getWord(%spawn, 3));
   %player.mountImage(ArenaPistolImage, 0);
   // The pistol is held up (the arm thread's `hold`): every client draws the figure as its record says.
   %player.setArmThread(hold);
   %player.ammo = $Arena::Rounds;
   %player.setImageAmmo(0, true);
   %client.sendOwnFigure(%player, 0);
   commandToClient(%client, 'SetAmmo', %player.ammo, $Arena::Rounds);
   commandToClient(%client, 'SetHealth', ArenaFigure.maxDamage, ArenaFigure.maxDamage);
}

// Puts the figure of the client named %name at %x %y %z facing %yaw (the pictures set their scenes this way).
function placeFigure(%name, %x, %y, %z, %yaw)
{
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
   {
      %client = ClientGroup.getObject(%i);
      if (%client.name $= %name && isObject(%client.player))
      {
         %client.player.setTransform(%x SPC %y SPC %z SPC "0 0 1" SPC %yaw);
         %client.sendOwnFigure(%client.player, 0);
      }
   }
}

// The figure of the client named %name (the pictures look at it on the server), or 0.
function figureOf(%name)
{
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
   {
      %client = ClientGroup.getObject(%i);
      if (%client.name $= %name && isObject(%client.player))
         return %client.player.getId();
   }
   return 0;
}

// Tells the client which ghost is its own figure, and which way it faces, once the figure's ghost has reached it (the view is
// drawn from the figure's eyes, turned as the client's own mouse turns it).
function GameConnection::sendOwnFigure(%client, %player, %tries)
{
   if (!isObject(%player) || %client.player != %player)
      return;
   %ghost = %client.getGhostID(%player);
   if (%ghost == -1)
   {
      if (%tries < 100)
         %client.schedule(100, "sendOwnFigure", %player, %tries + 1);
      return;
   }
   %t = %player.getTransform();
   commandToClient(%client, 'OwnFigure', %ghost, getWord(%t, 6) * getWord(%t, 5));
}

// ---- the pistol

// R: a reload, when the magazine is not full and none is running (the image's ammo cleared sends it to its Reload state).
function serverCmdReload(%client)
{
   %player = %client.player;
   if (!isObject(%player) || %player.getState() $= "Dead")
      return;
   if (%player.reloading || %player.ammo >= $Arena::Rounds)
      return;
   %player.setImageAmmo(0, false);
}

function ArenaWeapon::onReload(%this, %obj, %slot)
{
   %obj.reloading = true;
   if (isObject(%obj.client))
      commandToClient(%obj.client, 'Reloading', $Arena::ReloadSeconds);
}

function ArenaWeapon::onReloaded(%this, %obj, %slot)
{
   %obj.reloading = false;
   %obj.ammo = $Arena::Rounds;
   %obj.setImageAmmo(%slot, true);
   if (isObject(%obj.client))
      commandToClient(%obj.client, 'SetAmmo', %obj.ammo, $Arena::Rounds);
}

function ArenaWeapon::onFire(%this, %obj, %slot)
{
   // Between the round's end and the next map nobody shoots.
   if (%obj.ammo <= 0 || $Arena::RoundOver)
      return;
   %obj.ammo--;
   if (%obj.ammo <= 0)
      %obj.setImageAmmo(%slot, false);
   %client = %obj.client;
   if (isObject(%client))
      commandToClient(%client, 'SetAmmo', %obj.ammo, $Arena::Rounds);
   // The bolt flies along the eye's line (the crosshair) at every range: it starts on that line as far ahead of the eye as
   // the muzzle is, instead of at the muzzle beside it, whose corrected aim only meets the line where the eye's ray ends.
   // The steady eye (the head's rest pose, not its walking bob): where the client's view and crosshair are. From behind the
   // figure the crosshair is on the camera's line instead: the bolt goes to what that line meets (thirdPersonAim).
   %eye = %obj.getSteadyEyePoint();
   %aim = %obj.getEyeVector();
   if (isObject(%client) && %client.thirdPerson && !%obj.aiming)
      %aim = thirdPersonAim(%obj, %eye, %aim);
   %ahead = VectorDot(VectorSub(%obj.getMuzzlePoint(%slot), %eye), %aim);
   if (%ahead < 0.25)
      %ahead = 0.25;
   %bolt = new Projectile()
   {
      dataBlock = %this.projectile;
      initialVelocity = VectorScale(%aim, %this.projectile.muzzleVelocity);
      initialPosition = VectorAdd(%eye, VectorScale(%aim, %ahead));
      sourceObject = %obj;
      sourceSlot = %slot;
      client = %client;
   };
   MissionCleanup.add(%bolt);
}

// The camera behind the figure as the client places it (client.rs: CAMERA_DISTANCE back along the look, CAMERA_SIDE to the
// right, CAMERA_LIFT up, CAMERA_CLEARANCE in front of a wall between, and the first person's view with less than
// CAMERA_NEAREST of room), what it keeps clear of (the map's static shapes, terrain and interiors) and what its crosshair's
// line stops at (those and the figures), and how far that line reaches.
$Arena::CameraDistance = 3.5;
$Arena::CameraSide = 0.6;
$Arena::CameraLift = 0.45;
$Arena::CameraClearance = 0.3;
$Arena::CameraNearest = 0.8;
$Arena::CameraMask = 8205;
$Arena::AimMask = 24589;
$Arena::AimRange = 300;

// The direction from %eye (the figure %obj's steady eye, looking along %look) to what the crosshair of the camera behind the
// figure is over: the server places that camera itself, as the client does, and casts along its line, so a client says only
// which camera it looks through (serverCmdThirdPerson), never where it aims. A camera with no room behind the figure is the
// first person's, and a point no further ahead of the eye than a unit (the camera's line meeting something between the camera
// and the eye) keeps the eye's own line.
function thirdPersonAim(%obj, %eye, %look)
{
   %right = VectorNormalize(VectorCross(%look, "0 0 1"));
   %back = VectorAdd(VectorSub(%eye, VectorScale(%look, $Arena::CameraDistance)), VectorScale(%right, $Arena::CameraSide));
   %back = VectorAdd(%back, "0 0" SPC $Arena::CameraLift);
   %room = VectorDist(%eye, %back);
   %hit = containerRayCast(%eye, %back, $Arena::CameraMask, %obj);
   if (%hit)
      %room = VectorDist(%eye, getWords(%hit, 1, 3)) - $Arena::CameraClearance;
   if (%room < $Arena::CameraNearest)
      return %look;
   %camera = VectorAdd(%eye, VectorScale(VectorNormalize(VectorSub(%back, %eye)), %room));
   %end = VectorAdd(%camera, VectorScale(%look, $Arena::AimRange));
   %hit = containerRayCast(%camera, %end, $Arena::AimMask, %obj);
   %target = %hit ? getWords(%hit, 1, 3) : %end;
   %to = VectorSub(%target, %eye);
   if (VectorDot(%to, %look) < 1)
      return %look;
   return VectorNormalize(%to);
}

// Which camera the client looks through: behind its figure (Tab) or at its eye.
function serverCmdThirdPerson(%client, %on)
{
   %client.thirdPerson = %on ? 1 : 0;
}

function ArenaBolt::onCollision(%this, %bolt, %col, %fade, %pos, %normal)
{
   if (%col.getClassName() !$= "Player")
      return;
   // The part hit (head, torso, rarm, ...): every part takes the same damage; the shooter is told which.
   %part = %col.getDamageLocation(%pos);
   %col.getDataBlock().hurt(%col, %bolt.client, %this.directDamage, %part);
   // The figure flinches away from the shot (common/server.cs), its head snapped back for a head shot.
   playerFlinch(%col, %bolt.initialVelocity, %part);
}

function ArenaFigure::onEndSequence(%this, %obj, %slot)
{
   playerFlinchEnded(%obj, %slot);
}

// ---- hits and knock-outs

function ArenaFigure::hurt(%this, %obj, %attacker, %amount, %part)
{
   if (%obj.getState() $= "Dead" || $Arena::RoundOver)
      return;
   // Where the shot came from, for the fall (onDisabled): the shooter's figure in front of this one or behind it.
   %obj.hitFromFront = true;
   if (isObject(%attacker) && isObject(%attacker.player) && %attacker.player != %obj)
   {
      %to = VectorSub(%attacker.player.getPosition(), %obj.getPosition());
      %obj.hitFromFront = VectorDot(%to, %obj.getForwardVector()) >= 0;
   }
   %obj.applyDamage(%amount);
   if (%part !$= "")
      echo(%obj.getShapeName() SPC "hit in the" SPC %part @ ".");
   %client = %obj.client;
   %left = %this.maxDamage - %obj.getDamageLevel();
   if (%left < 0)
      %left = 0;
   if (isObject(%client))
      commandToClient(%client, 'SetHealth', mFloor(%left), %this.maxDamage);
   // Where the hurt came from: the client's screen shows which way.
   if (isObject(%client) && isObject(%attacker) && isObject(%attacker.player))
      commandToClient(%client, 'Hurt', %amount, %attacker.player.getPosition());
   if (isObject(%attacker) && %attacker != %client)
      commandToClient(%attacker, 'Hit', %client.name, %part);
   if (%left <= 0)
      knockOut(%client, %attacker);
}

// Aiming down the sights (the move's trigger 4, the right mouse button held): the figure raises its pistol to its eye, which
// the others see; let go, it holds it as before.
function ArenaFigure::onTrigger(%this, %obj, %slot, %down)
{
   if (%slot != 4 || %obj.getState() $= "Dead")
      return;
   // Aiming from behind the figure, the client's view is the first person's at the sights (and so is the shot's line).
   %obj.aiming = %down ? 1 : 0;
   %obj.setArmThread(%down ? "aim" : "hold");
}

// Knocked out (the damage reached maxDamage): the figure falls away from the shot, backward when hit from the front, and lies
// still on the last frame until it respawns. Its pistol goes (the respawned figure mounts a new one): nothing is held before
// a knocked-out player's eye or in a fallen figure's hand.
function ArenaFigure::onDisabled(%this, %obj, %state)
{
   %obj.setActionThread(%obj.hitFromFront ? "death1" : "death2", true);
   %obj.unmountImage(0);
}

function knockOut(%client, %attacker)
{
   if (!isObject(%client))
      return;
   if (isObject(%attacker) && %attacker != %client)
   {
      %attacker.score++;
      messageAll("info", %attacker.name SPC "knocked out" SPC %client.name @ ".");
   }
   else
      messageAll("info", %client.name SPC "was knocked out.");
   commandToClient(%client, 'KnockedOut', $Arena::RespawnMs);
   %client.respawn = %client.schedule($Arena::RespawnMs, "spawnFigure");
   // The score limit ends the round at once (after the respawn is scheduled: endRound cancels it).
   if (isObject(%attacker) && %attacker != %client && $Pref::Server::ScoreLimit > 0 && %attacker.score >= $Pref::Server::ScoreLimit)
   {
      messageAll("info", %attacker.name SPC "reached the score limit of" SPC $Pref::Server::ScoreLimit @ ".");
      endRound();
   }
}

// A figure that fell off the map starts again.
function GameConnection::onFellOff(%client)
{
   %client.spawnFigure();
}

// ---- rounds and the map rotation
//
// A round lasts $Pref::Server::RoundSeconds (five minutes), or until a player's score reaches $Pref::Server::ScoreLimit (10;
// 0 turns the limit off). Then the shooting stops, every client is shown the round's scores and the next map's name for
// $Pref::Server::RoundEndSeconds, and the next map loads (each time pref held to at least $Arena::MinRoundSeconds and
// $Arena::MinRoundEndSeconds): the clients stay connected and download it as they downloaded the first, and every score
// starts again at 0. The host can load a map of the list at once from the pause menu (serverCmdChangeMap). The maps go around
// $Pref::Server::MapList (paths in the game folder, separated by spaces), or when it is empty every map that
// $Server::MissionFileSpec finds, in order of their names.
//
// Credit: the round's timer, its end and the next map follow the cycle of TGE's starter game scripts as Torque3D carries them
// (MIT: `core/scripts/server/game.cs`, `startGame` / `onGameDurationEnd` / `cycleGame` / `onCyclePauseEnd`, Copyright (c)
// 2012 GarageGames, LLC). The code here is the arena's own.

if ($Pref::Server::RoundSeconds $= "")
   $Pref::Server::RoundSeconds = 300;
if ($Pref::Server::ScoreLimit $= "")
   $Pref::Server::ScoreLimit = 10;
if ($Pref::Server::RoundEndSeconds $= "")
   $Pref::Server::RoundEndSeconds = 8;
$Server::MissionFileSpec = "missions/*.mis";
// The shortest round and pause the prefs may ask for (a test lowers them).
$Arena::MinRoundSeconds = 30;
$Arena::MinRoundEndSeconds = 1;
// The least time between two map changes the host asks for.
$Arena::MapChangeGapMs = 3000;

// The round's length and the pause after it in ms, as the prefs ask but no shorter than the least allowed.
function roundMs()
{
   %s = $Pref::Server::RoundSeconds;
   return mFloor((%s < $Arena::MinRoundSeconds ? $Arena::MinRoundSeconds : %s) * 1000);
}

function roundEndMs()
{
   %s = $Pref::Server::RoundEndSeconds;
   return mFloor((%s < $Arena::MinRoundEndSeconds ? $Arena::MinRoundEndSeconds : %s) * 1000);
}

// A new round on the map that runs: the scores at 0 and the clock started.
function startRound()
{
   cancel($Arena::RoundTimer);
   $Arena::RoundOver = false;
   $Arena::Round++;
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
   {
      %client = ClientGroup.getObject(%i);
      %client.score = 0;
   }
   %ms = roundMs();
   $Arena::RoundEndsAt = getSimTime() + %ms;
   $Arena::RoundTimer = schedule(%ms, 0, "endRound");
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
      commandToClient(ClientGroup.getObject(%i), 'RoundTime', roundSecondsLeft());
}

// The seconds left in the round (0 once it is over).
function roundSecondsLeft()
{
   if ($Arena::RoundOver)
      return 0;
   %left = mCeil(($Arena::RoundEndsAt - getSimTime()) / 1000);
   return %left < 0 ? 0 : %left;
}

// The round is over: the scores to everyone, and the next map after a pause.
function endRound()
{
   // Asked again during the pause (the next map is already timed): nothing more.
   if ($Arena::RoundOver)
      return;
   cancel($Arena::RoundTimer);
   $Arena::RoundOver = true;
   $Arena::NextMission = nextMission();
   %board = scoreBoard();
   %next = fileBase($Arena::NextMission);
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
   {
      %client = ClientGroup.getObject(%i);
      cancel(%client.respawn);
      commandToClient(%client, 'RoundEnd', %next, mCeil(roundEndMs() / 1000), %board);
   }
   %top = getRecord(%board, 0);
   if (%top !$= "" && getField(%top, 1) > 0)
      messageAll("info", getField(%top, 0) SPC "wins the round on" SPC $Arena::Title SPC "with" SPC getField(%top, 1) @ ".");
   else
      messageAll("info", "The round on" SPC $Arena::Title SPC "is over.");
   echo("Round over on" SPC $Arena::Title @ "; next:" SPC $Arena::NextMission);
   $Arena::RoundTimer = schedule(roundEndMs(), 0, "cycleMissions");
}

// Loads the map the round's end named.
function cycleMissions()
{
   %file = $Arena::NextMission;
   if (%file $= "")
      %file = nextMission();
   $Arena::NextMission = "";
   startArena(%file);
}

// The maps the rotation goes around and the host may pick from, as paths separated by spaces: $Pref::Server::MapList, or
// when it is empty every map the game folder has, in order of their names.
function mapList()
{
   %list = trim($Pref::Server::MapList);
   if (%list !$= "")
      return %list;
   // In order of their names (the search finds them in the file host's own order).
   %found = 0;
   for (%file = findFirstFile($Server::MissionFileSpec); %file !$= ""; %file = findNextFile($Server::MissionFileSpec))
   {
      for (%i = %found; %i > 0 && stricmp(%maps[%i - 1], %file) > 0; %i--)
         %maps[%i] = %maps[%i - 1];
      %maps[%i] = %file;
      %found++;
   }
   for (%i = 0; %i < %found; %i++)
      %list = %list SPC %maps[%i];
   return trim(%list);
}

// The map after the one that runs, in the rotation's list.
function nextMission()
{
   %list = mapList();
   %count = getWordCount(%list);
   if (%count == 0)
      return $Server::MissionFile;
   for (%i = 0; %i < %count; %i++)
      if (getWord(%list, %i) $= $Server::MissionFile)
         return getWord(%list, (%i + 1) % %count);
   return getWord(%list, 0);
}

// ---- the host's map change

// Tells the host's client the maps it may pick (the pause menu's Change map).
function sendMapChoices(%client)
{
   %list = mapList();
   %count = getWordCount(%list);
   commandToClient(%client, 'MapChoices', %count);
   for (%i = 0; %i < %count; %i++)
      commandToClient(%client, 'MapChoice', getWord(%list, %i));
}

// The host picked %name from the pause menu: that map loads at once, every client stays connected and the scores start
// again. Only the host's own connection may ask (the server marked it when it connected, by the key only the host's process
// holds), and only for a map of the server's own list: the path that loads is the list's, never the client's text.
// A refused map change, logged; a client other than the host is logged at most once every five seconds, so one sending it over
// and over can't fill the server's log.
function refuseMapChange(%client, %why)
{
   %now = getSimTime();
   if (!%client.isHost && %client.mapRefusedAt !$= "" && %now - %client.mapRefusedAt < 5000)
      return;
   %client.mapRefusedAt = %now;
   echo("Refused a map change from" SPC %client SPC "(" @ %client.name @ "):" SPC %why);
}

function serverCmdChangeMap(%client, %name)
{
   if (!%client.isHost)
   {
      refuseMapChange(%client, "not the host.");
      return;
   }
   %name = trim(%name);
   %list = mapList();
   %file = "";
   for (%i = 0; %i < getWordCount(%list); %i++)
      if (%name $= getWord(%list, %i))
         %file = getWord(%list, %i);
   if (%file $= "")
   {
      refuseMapChange(%client, "not in the map list.");
      return;
   }
   %now = getSimTime();
   if ($Arena::MapChangeAt !$= "" && %now - $Arena::MapChangeAt < $Arena::MapChangeGapMs)
   {
      refuseMapChange(%client, "too soon after the last.");
      return;
   }
   $Arena::MapChangeAt = %now;
   messageAll("info", %client.name SPC "changed the map to" SPC fileBase(%file) @ ".");
   echo("Map change to" SPC %file SPC "by the host.");
   // Like the round's end, without the pause: nobody respawns into the map that is going.
   cancel($Arena::RoundTimer);
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
      cancel(ClientGroup.getObject(%i).respawn);
   $Arena::RoundOver = true;
   $Arena::NextMission = %file;
   // From the next event, not from inside the packet that carried the command.
   $Arena::RoundTimer = schedule(0, 0, "cycleMissions");
}

// The round's scores, the highest first: a record ("name" TAB score) for each of the first eight players.
function scoreBoard()
{
   %n = ClientGroup.getCount();
   for (%i = 0; %i < %n; %i++)
      %order[%i] = ClientGroup.getObject(%i);
   for (%i = 0; %i < %n; %i++)
      for (%j = %i + 1; %j < %n; %j++)
         if (%order[%j].score > %order[%i].score)
         {
            %t = %order[%i];
            %order[%i] = %order[%j];
            %order[%j] = %t;
         }
   %board = "";
   for (%i = 0; %i < %n && %i < 8; %i++)
      %board = %board @ (%i > 0 ? "\n" : "") @ %order[%i].name TAB %order[%i].score;
   return %board;
}
