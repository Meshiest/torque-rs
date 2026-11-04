// The arena's server. The host sets $Pref::Server::Name, execs this file and calls startArena(%file) with the map to play.
// A client that connects is sent the datablocks; when it has them, its ghosting starts and it gets a figure holding a pistol
// with an 11-round magazine, reloaded as often as it likes (R, or by itself when empty). Bolts hurt figures; three hits knock
// one out: it falls over, away from the shot, and comes back at a spawn point a moment later.

// ---- datablocks

$Arena::ReloadSeconds = 1.2;

datablock StaticShapeData(ArenaCube)
{
   category = "Arena";
   shapeFile = "kit/cube.dts";
};

datablock PlayerData(ArenaFigure)
{
   category = "Arena";
   shapeFile = "kit/figure.dts";

   mass = 90;
   drag = 0.3;
   maxDamage = 100;
   boundingBox = "1 1 2";

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
};

// The bolt's trail: small hot sparks added over what is behind them, fading from yellow through orange to nothing as they
// shrink. A particle's life goes to the client in steps of 32 ms.
datablock ParticleData(ArenaBoltSpark)
{
   textureName = "kit/spark";
   dragCoefficient = 2;
   gravityCoefficient = 0;
   inheritedVelFactor = 0;
   lifetimeMS = 64;
   lifetimeVarianceMS = 32;
   useInvAlpha = false;
   spinRandomMin = -90;
   spinRandomMax = 90;

   colors[0] = "1 0.9 0.5 0.8";
   colors[1] = "1 0.55 0.12 0.5";
   colors[2] = "0.9 0.25 0.05 0";
   sizes[0] = 0.24;
   sizes[1] = 0.14;
   sizes[2] = 0.05;
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

datablock ProjectileData(ArenaBolt)
{
   explosion = ArenaBoltHit;
   projectileShapeName = "kit/projectile.dts";
   particleEmitter = ArenaBoltTrail;
   directDamage = 34;
   muzzleVelocity = 60;
   velInheritFactor = 0;
   armingDelay = 0;
   lifetime = 3000;
   fadeDelay = 2800;
   isBallistic = false;
};

// The muzzle flash: a quick burst of large, hot sparks at the muzzle, added over what is behind them, gone in a few frames.
datablock ParticleData(ArenaMuzzleSpark)
{
   textureName = "kit/spark";
   dragCoefficient = 8;
   gravityCoefficient = 0;
   inheritedVelFactor = 0;
   lifetimeMS = 64;
   lifetimeVarianceMS = 0;
   useInvAlpha = false;
   spinRandomMin = -180;
   spinRandomMax = 180;

   colors[0] = "1 0.95 0.75 1";
   colors[1] = "1 0.7 0.25 0.8";
   colors[2] = "1 0.4 0.1 0";
   sizes[0] = 0.22;
   sizes[1] = 0.3;
   sizes[2] = 0.12;
   times[0] = 0;
   times[1] = 0.3;
   times[2] = 1;
};

datablock ParticleEmitterData(ArenaMuzzleFlash)
{
   ejectionPeriodMS = 4;
   periodVarianceMS = 0;
   ejectionVelocity = 2;
   velocityVariance = 1;
   thetaMin = 0;
   thetaMax = 25;
   phiReferenceVel = 0;
   phiVariance = 360;
   particles = "ArenaMuzzleSpark";
};

datablock ShapeBaseImageData(ArenaPistolImage)
{
   shapeFile = "kit/pistol.dts";
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
   stateScript[2] = "onFire";
   stateEmitter[2] = ArenaMuzzleFlash;
   stateEmitterTime[2] = 0.05;
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

new SimGroup(ClientGroup);

$Arena::Rounds = 11;
$Arena::RespawnMs = 2500;
$Arena::Sequence = 0;
$Arena::FallHeight = -30;

// A packet to each client every tick (TGE's most; its default is ten a second): the other figures move on smaller steps.
$pref::Net::PacketRateToClient = 32;

$Server::GameType = "Arena";
$Server::MissionType = "Free for all";
$Server::PlayerCount = 0;
$Pref::Server::MaxPlayers = 16;
if ($Pref::Server::Name $= "")
   $Pref::Server::Name = "Arena";

// Loads the map %file: a script that builds MissionGroup and sets $Arena::SpawnCount and $Arena::Spawn[i] ("x y z yaw").
// MissionGroup's textureFilter field ("nearest", or none) is how the clients draw the map's textures.
function startArena(%file)
{
   $Arena::Sequence++;
   if (isObject(MissionGroup))
      MissionGroup.delete();
   if (isObject(MissionCleanup))
      MissionCleanup.delete();
   new SimGroup(MissionCleanup);
   $Arena::SpawnCount = 0;
   $Arena::Title = fileBase(%file);
   exec(%file);
   if ($Arena::SpawnCount == 0)
   {
      $Arena::Spawn[0] = "0 0 1 0";
      $Arena::SpawnCount = 1;
   }
   $Server::MissionName = $Arena::Title;
   echo("Arena" SPC $Arena::Title SPC "with" SPC MissionGroup.getCount() SPC "objects and" SPC $Arena::SpawnCount SPC "spawn points");
   schedule(500, 0, "watchFalls");
}

// The LAN list's line about this game.
function onServerInfoQuery()
{
   return "In the arena";
}

// ---- messages to every client

// %kind is "chat" (a player's line: %a is the name, %b the text) or "info" (a line from the arena: %a).
function messageAll(%kind, %a, %b)
{
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
      commandToClient(ClientGroup.getObject(%i), 'ArenaMessage', %kind, %a, %b);
}

// ---- a client's way in

function GameConnection::onConnect(%client, %name)
{
   %name = trim(stripChars(%name, "\t\n\r"));
   if (%name $= "")
      %name = "Player" SPC %client;
   %client.name = getSubStr(%name, 0, 24);
   %client.score = 0;
   $Server::PlayerCount++;
   echo("Client" SPC %client SPC "joined as" SPC %client.name);
   %client.transmitDataBlocks($Arena::Sequence);
}

function GameConnection::onDataBlocksDone(%client, %sequence)
{
   if (%sequence != $Arena::Sequence)
      return;
   %client.activateGhosting();
   %client.spawnFigure();
   commandToClient(%client, 'ArenaStarted', $Arena::Title, $Pref::Server::Name, MissionGroup.textureFilter);
   messageAll("info", %client.name SPC "joined the arena.");
}

function GameConnection::onDrop(%client, %reason)
{
   echo("Client" SPC %client SPC "left:" SPC %reason);
   if (isObject(%client.player))
      %client.player.delete();
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
   if (isObject(%client.player))
      %client.player.delete();
   %player = new Player()
   {
      dataBlock = ArenaFigure;
      client = %client;
   };
   MissionCleanup.add(%player);
   %spawn = nextSpawn();
   %player.setTransform(getWords(%spawn, 0, 2) SPC "0 0 1" SPC getWord(%spawn, 3));
   %player.setShapeName(%client.name);
   %player.mountImage(ArenaPistolImage, 0);
   %player.ammo = $Arena::Rounds;
   %player.setImageAmmo(0, true);
   %client.player = %player;
   %client.setControlObject(%player);
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
   if (%obj.ammo <= 0)
      return;
   %obj.ammo--;
   if (%obj.ammo <= 0)
      %obj.setImageAmmo(%slot, false);
   %client = %obj.client;
   if (isObject(%client))
      commandToClient(%client, 'SetAmmo', %obj.ammo, $Arena::Rounds);
   // The bolt flies along the eye's line (the crosshair) at every range: it starts on that line as far ahead of the eye as
   // the muzzle is, instead of at the muzzle beside it, whose corrected aim only meets the line where the eye's ray ends.
   %eye = %obj.getEyePoint();
   %aim = %obj.getEyeVector();
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

function ArenaBolt::onCollision(%this, %bolt, %col, %fade, %pos, %normal)
{
   if (%col.getClassName() !$= "Player")
      return;
   %col.getDataBlock().hurt(%col, %bolt.client, %this.directDamage);
}

// ---- hits and knock-outs

function ArenaFigure::hurt(%this, %obj, %attacker, %amount)
{
   if (%obj.getState() $= "Dead")
      return;
   // Where the shot came from, for the fall (onDisabled): the shooter's figure in front of this one or behind it.
   %obj.hitFromFront = true;
   if (isObject(%attacker) && isObject(%attacker.player) && %attacker.player != %obj)
   {
      %to = VectorSub(%attacker.player.getPosition(), %obj.getPosition());
      %obj.hitFromFront = VectorDot(%to, %obj.getForwardVector()) >= 0;
   }
   %obj.applyDamage(%amount);
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
      commandToClient(%attacker, 'Hit', %client.name);
   if (%left <= 0)
      knockOut(%client, %attacker);
}

// Knocked out (the damage reached maxDamage): the figure falls away from the shot, backward when hit from the front, and lies
// still on the last frame until it respawns.
function ArenaFigure::onDisabled(%this, %obj, %state)
{
   %obj.setActionThread(%obj.hitFromFront ? "death1" : "death2", true);
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
}

// A figure that fell off the map starts again.
function watchFalls()
{
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
   {
      %client = ClientGroup.getObject(%i);
      if (isObject(%client.player) && getWord(%client.player.getTransform(), 2) < $Arena::FallHeight)
         %client.spawnFigure();
   }
   schedule(500, 0, "watchFalls");
}

// ---- chat

function serverCmdSay(%client, %text)
{
   %text = trim(stripChars(%text, "\t\n\r"));
   if (%text $= "")
      return;
   messageAll("chat", %client.name, getSubStr(%text, 0, 120));
}
