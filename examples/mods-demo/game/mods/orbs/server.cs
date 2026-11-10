// The orbs mod's datablocks and the few helpers its Lua rules call: Lua cannot declare a datablock or make an object of its
// own, so the declarations and the `new` calls are here, and the rules (server.lua) are the game mode.

datablock ItemData(OrbData)
{
   shapeFile = "shapes/orb.dts";
   category = "Orbs";
};

datablock ItemData(RunnerAData)
{
   shapeFile = "shapes/runner_a.dts";
   category = "Orbs";
};

datablock ItemData(RunnerBData)
{
   shapeFile = "shapes/runner_b.dts";
   category = "Orbs";
};

// The rope from a runner to the orb it chases: a glowing tube drawn with the mod's picture, which scrolls along it.
datablock RopeData(OrbBeam)
{
   segments = 6;
   texture = "art/beam.png";
   textureLength = 1.5;
   textureScroll = 1.5;
   thickness = 0.14;
   color = "255 255 255 230";
   emissive = true;
};

// The leader's tether: a thicker cable up to a point over the field.
datablock RopeData(OrbCrown)
{
   segments = 12;
   thickness = 0.09;
   color = "255 215 80 255";
   emissive = true;
};

function orbsSpawn(%data, %x, %y, %z, %spin)
{
   if (!isObject(MissionCleanup))
      new SimGroup(MissionCleanup);
   %item = new Item()
   {
      dataBlock = %data;
      position = %x SPC %y SPC %z;
      static = true;
      rotate = %spin;
   };
   MissionCleanup.add(%item);
   return %item;
}

function orbsMove(%item, %x, %y, %z)
{
   %item.setTransform(%x SPC %y SPC %z SPC "1 0 0 0");
}

function orbsRope(%data)
{
   %rope = ropeNetCreate("0 0 0", "0 0 1", %data);
   MissionCleanup.add(%rope);
   return %rope;
}

function orbsAnnounce(%client, %state)
{
   commandToClient(%client, 'OrbsState', %state);
}
