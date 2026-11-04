// The arena's server. The host sets $Pref::Server::Name, execs this file and calls startArena(%file) with the map to play.
// A client that connects is sent the datablocks; when it has them, its ghosting starts and it gets a figure holding a pistol
// with 11 rounds. Bolts hurt figures; three hits knock one out, and it comes back at a spawn point a moment later.

// ---- datablocks

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

datablock ProjectileData(ArenaBolt)
{
   projectileShapeName = "kit/projectile.dts";
   directDamage = 34;
   muzzleVelocity = 60;
   velInheritFactor = 0;
   armingDelay = 0;
   lifetime = 3000;
   fadeDelay = 2800;
   isBallistic = false;
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

   stateName[1] = "Ready";
   stateTransitionOnNoAmmo[1] = "Empty";
   stateTransitionOnTriggerDown[1] = "Fire";

   stateName[2] = "Fire";
   stateFire[2] = true;
   stateAllowImageChange[2] = false;
   stateSequence[2] = "fire";
   stateScript[2] = "onFire";
   stateTimeoutValue[2] = 0.15;
   stateTransitionOnTimeout[2] = "Release";

   // One round a click: the trigger has to come up before the next.
   stateName[3] = "Release";
   stateTransitionOnTriggerUp[3] = "Ready";

   // Out of rounds: clicking does nothing.
   stateName[4] = "Empty";
   stateTransitionOnAmmo[4] = "Ready";
};

// ---- the arena

new SimGroup(ClientGroup);

$Arena::Rounds = 11;
$Arena::RespawnMs = 2500;
$Arena::Sequence = 0;
$Arena::FallHeight = -30;

$Server::GameType = "Arena";
$Server::MissionType = "Free for all";
$Server::PlayerCount = 0;
$Pref::Server::MaxPlayers = 16;
if ($Pref::Server::Name $= "")
   $Pref::Server::Name = "Arena";

// Loads the map %file: a script that builds MissionGroup and sets $Arena::SpawnCount and $Arena::Spawn[i] ("x y z yaw").
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
   commandToClient(%client, 'ArenaStarted', $Arena::Title, $Pref::Server::Name);
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

// The next spawn point, round the map's list.
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
   %bolt = new Projectile()
   {
      dataBlock = %this.projectile;
      initialVelocity = VectorScale(%obj.getMuzzleVector(%slot), %this.projectile.muzzleVelocity);
      initialPosition = %obj.getMuzzlePoint(%slot);
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
   %obj.applyDamage(%amount);
   %client = %obj.client;
   %left = %this.maxDamage - %obj.getDamageLevel();
   if (%left < 0)
      %left = 0;
   if (isObject(%client))
      commandToClient(%client, 'SetHealth', mFloor(%left), %this.maxDamage);
   if (isObject(%attacker) && %attacker != %client)
      commandToClient(%attacker, 'Hit', %client.name);
   if (%left <= 0)
      knockOut(%client, %attacker);
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
