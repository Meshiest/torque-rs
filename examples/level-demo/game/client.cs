// The level demo's client: the menu of maps, the view of the world with its help line, the keys, and the way in.
//
// The host fills $World::MapCount, $World::MapTitle[i] and $World::MapAbout[i], then calls buildMenu(). A map picked sets
// $World::Pick; the host starts that map's server and calls connectTo(%address). Leaving sets $World::Leave.

exec("common/client.cs");

// ---- profiles

new GuiControlProfile(WorldBackProfile) { opaque = true; fillColor = "18 22 30 255"; };
new GuiControlProfile(WorldBandProfile) { opaque = true; fillColor = "12 16 24 160"; };
new GuiControlProfile(WorldTitleProfile) { fontType = "Arial"; fontSize = 40; fontColors[0] = "240 242 248 255"; };
new GuiControlProfile(WorldTextProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "222 226 236 255"; };
new GuiControlProfile(WorldDimProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "150 160 178 255"; };
new GuiControlProfile(WorldNoteProfile) { fontType = "Arial"; fontSize = 22; fontColors[0] = "255 214 110 255"; justify = "center"; };
new GuiControlProfile(WorldButtonProfile)
{
   opaque = true;
   border = true;
   fillColor = "44 54 72 255";
   fillColorHL = "70 92 128 255";
   fillColorNA = "30 34 42 255";
   borderColor = "96 116 150 255";
   fontType = "Arial";
   fontSize = 20;
   fontColors[0] = "236 240 248 255";
   fontColors[1] = "255 255 255 255";
   fontColors[2] = "120 126 138 255";
   justify = "center";
   canKeyFocus = true;
};

createCanvas("torque-rs level demo");

// ---- the menu

new GuiControl(MenuGui)
{
   profile = WorldBackProfile; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiTextCtrl(MenuTitle)
   {
      profile = WorldTitleProfile; position = "60 50"; extent = "680 48"; visible = true; enabled = true;
      text = "Level Demo";
   };
   new GuiTextCtrl(MenuLine)
   {
      profile = WorldDimProfile; position = "62 104"; extent = "680 20"; visible = true; enabled = true;
      text = "Pick a map to walk around in. Click it, or press its number.";
   };
   new GuiControl(MenuList) { position = "60 150"; extent = "680 400"; horizSizing = "width"; };
   new GuiTextCtrl(MenuStatus)
   {
      profile = WorldNoteProfile; position = "60 540"; extent = "680 26"; vertSizing = "top"; visible = true; enabled = true;
      text = "";
   };
};

// One button and one line for each map the host found.
function buildMenu()
{
   MenuList.clear();
   if ($World::MapCount == 0)
   {
      MenuStatus.setText("No maps found in the missions folder.");
      return;
   }
   for (%i = 0; %i < $World::MapCount; %i++)
   {
      %y = %i * 84;
      MenuList.add(new GuiButtonCtrl("MapButton" @ %i)
      {
         profile = WorldButtonProfile; position = "0" SPC %y; extent = "260 56"; visible = true; enabled = true;
         text = (%i + 1) @ "   " @ $World::MapTitle[%i];
         command = "pickMap(" @ %i @ ");";
      });
      // The line wraps in the width the window leaves, up to three lines beside the button.
      MenuList.add(new GuiMLTextCtrl("MapAbout" @ %i)
      {
         profile = WorldTextProfile; position = "284" SPC %y + 10; extent = "396 72"; horizSizing = "width"; visible = true;
         enabled = true; text = $World::MapAbout[%i];
      });
   }
}

function pickMap(%i)
{
   if (%i < 0 || %i >= $World::MapCount)
      return;
   MenuStatus.setText("Loading " @ $World::MapTitle[%i] @ "...");
   $World::Pick = %i;
}

// ---- the world's view

new GameTSCtrl(PlayGui)
{
   position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(PlayBand)
   {
      profile = WorldBandProfile; position = "0 0"; extent = "800 28"; horizSizing = "width";
      new GuiTextCtrl(PlayTitle)
      {
         profile = WorldTextProfile; position = "10 5"; extent = "160 18"; visible = true; enabled = true; text = "";
      };
      new GuiTextCtrl(PlayHelp)
      {
         profile = WorldDimProfile; position = "170 5"; extent = "620 18"; horizSizing = "width"; visible = true; enabled = true;
         text = "W A S D move   Shift walk   Space jump   Mouse look   V camera   R restart   Esc menu";
      };
   };
   new GuiTextCtrl(PlayNote)
   {
      profile = WorldNoteProfile; position = "100 80"; extent = "600 28"; horizSizing = "center"; visible = true; enabled = true;
      text = "";
   };
};


// ---- the keys

function resetCanvas() {}

new ActionMap(MenuMap);
function menuKey(%i, %down) { if (%down) pickMap(%i); }
function menuOne(%v) { menuKey(0, %v); }
function menuTwo(%v) { menuKey(1, %v); }
function menuThree(%v) { menuKey(2, %v); }
function menuFour(%v) { menuKey(3, %v); }
function menuFive(%v) { menuKey(4, %v); }
function menuSix(%v) { menuKey(5, %v); }
function menuSeven(%v) { menuKey(6, %v); }
function menuEight(%v) { menuKey(7, %v); }
function menuNine(%v) { menuKey(8, %v); }
function menuQuit(%v) { if (%v) quit(); }
MenuMap.bind(keyboard, 1, menuOne);
MenuMap.bind(keyboard, 2, menuTwo);
MenuMap.bind(keyboard, 3, menuThree);
MenuMap.bind(keyboard, 4, menuFour);
MenuMap.bind(keyboard, 5, menuFive);
MenuMap.bind(keyboard, 6, menuSix);
MenuMap.bind(keyboard, 7, menuSeven);
MenuMap.bind(keyboard, 8, menuEight);
MenuMap.bind(keyboard, 9, menuNine);
MenuMap.bind(keyboard, escape, menuQuit);

new ActionMap(PlayMap);
$World::Pace = 1;
function moveForward(%v) { $World::Forward = %v; updateMove(); }
function moveBack(%v) { $World::Back = %v; updateMove(); }
function moveLeft(%v) { $World::Left = %v; updateMove(); }
function moveRight(%v) { $World::Right = %v; updateMove(); }
function walkSlowly(%v) { $World::Pace = %v ? 0.35 : 1; updateMove(); }
function updateMove()
{
   $mvForwardAction = $World::Forward * $World::Pace;
   $mvBackwardAction = $World::Back * $World::Pace;
   $mvLeftAction = $World::Left * $World::Pace;
   $mvRightAction = $World::Right * $World::Pace;
}
function jump(%v) { if (%v) $mvTriggerCount2++; }
function turn(%v) { $mvYaw += %v * 0.003; }
function look(%v) { $mvPitch += %v * 0.003; }
function switchCamera(%v) { if (%v) $World::FirstPerson = !$World::FirstPerson; }
function restart(%v) { if (%v && isObject(ServerConnection)) commandToServer('Respawn'); }
function leaveKey(%v) { if (%v) leaveWorld(); }
PlayMap.bind(keyboard, w, moveForward);
PlayMap.bind(keyboard, s, moveBack);
PlayMap.bind(keyboard, a, moveLeft);
PlayMap.bind(keyboard, d, moveRight);
PlayMap.bind(keyboard, lshift, walkSlowly);
PlayMap.bind(keyboard, space, jump);
PlayMap.bind(keyboard, v, switchCamera);
PlayMap.bind(keyboard, r, restart);
PlayMap.bind(keyboard, escape, leaveKey);
PlayMap.bind(mouse, xaxis, turn);
PlayMap.bind(mouse, yaxis, look);

// ---- in and out

function showMenu()
{
   $World::InWorld = 0;
   PlayMap.pop();
   MenuMap.push();
   Canvas.setContent(MenuGui);
   Canvas.cursorOn();
}

function leaveWorld()
{
   disconnect();
   $World::Forward = $World::Back = $World::Left = $World::Right = 0;
   updateMove();
   MenuStatus.setText("");
   showMenu();
   $World::Leave = 1;
}

function onDataBlockObjectReceived(%index, %total)
{
   MenuStatus.setText("Loading the world... " @ %index + 1 @ " of " @ %total);
}

function onGhostAlwaysStarted(%count)
{
   MenuStatus.setText("Loading the world...");
}

function GameConnection::onConnectionAccepted(%this)
{
   MenuStatus.setText("Joined. Waiting for the world...");
}

function GameConnection::onConnectRequestRejected(%this, %reason)
{
   MenuStatus.setText("The world turned us away: " @ %reason);
}

function GameConnection::onConnectionDropped(%this, %reason)
{
   leaveWorld();
   MenuStatus.setText("Left the world: " @ %reason);
}

// An event for the page of a web build (`torque.on(name, ...)` there; the page bridge's `callPage`), with a fixed name: what the
// server sends is only ever an argument. Off the web there is no page and nothing happens.
function pageEvent(%name, %arg)
{
   if (isFunction("callPage"))
      callPage(%name, %arg);
}

function clientCmdWorldStarted(%title)
{
   pageEvent("mapLoaded", %title);
   PlayTitle.setText(%title);
   MenuMap.pop();
   PlayMap.push();
   Canvas.setContent(PlayGui);
   Canvas.cursorOff();
   $World::InWorld = 1;
   showNote(%title);
}

function clientCmdFell()
{
   pageEvent("playerFell");
   showNote("You fell off. Back to the start.");
}

function clientCmdRespawned()
{
   showNote("Back to the start.");
}

showMenu();
