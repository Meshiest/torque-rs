// The level demo's client: the menu of maps, the view of the world with its title and readout, the keys, the pause menu
// (common/pause.cs) with the list of keys, and the way in.
//
// The host fills $World::MapCount, $World::MapTitle[i] and $World::MapAbout[i], then calls buildMenu(). A map picked sets
// $World::Pick; the host starts that map's server and calls connectTo(%address). Leaving sets $World::Leave.

exec("common/client.cs");

// Ask the server for a packet every tick, as large as the wire's rate record allows: the link is in memory, and a map of many
// objects (the valley's trees) is ghosted in a few seconds instead of a minute.
$pref::Net::PacketRateToClient = 32;
$pref::Net::PacketSize = 1023;

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
exec("common/pause.cs");

// ---- the menu

new GuiControl(MenuGui)
{
   profile = WorldBackProfile; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   // the menu's column, centered in the window at any size
   new GuiControl(MenuPanel)
   {
      position = "60 42"; extent = "680 516"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(MenuTitle)
      {
         profile = WorldTitleProfile; position = "0 0"; extent = "680 48"; visible = true; enabled = true;
         text = "Level Demo";
      };
      new GuiTextCtrl(MenuLine)
      {
         profile = WorldDimProfile; position = "2 54"; extent = "678 20"; visible = true; enabled = true;
         text = "Pick a map to walk around in. Click it, or press its number.";
      };
      new GuiControl(MenuList) { position = "0 100"; extent = "680 384"; };
      new GuiTextCtrl(MenuStatus)
      {
         profile = WorldNoteProfile; position = "0 490"; extent = "680 26"; visible = true; enabled = true;
         text = "";
      };
   };
};

// Gives `%panel` the size `%w` by `%h`, centered in `%parent` (its horizSizing and vertSizing "center" keep it there as the
// window changes).
function fitPanel(%panel, %parent, %w, %h)
{
   %size = %parent.getExtent();
   %panel.resize(mFloor((getWord(%size, 0) - %w) / 2), mFloor((getWord(%size, 1) - %h) / 2), %w, %h);
}

// One button and one line for each map the host found; the panel fits the list, centered.
function buildMenu()
{
   MenuList.clear();
   if ($World::MapCount == 0)
   {
      MenuStatus.setText("No maps found in the missions folder.");
      return;
   }
   %rows = $World::MapCount * 80;
   MenuList.resize(0, 100, 680, %rows);
   MenuStatus.resize(0, 106 + %rows, 680, 26);
   fitPanel(MenuPanel, MenuGui, 680, 132 + %rows);
   for (%i = 0; %i < $World::MapCount; %i++)
   {
      %y = %i * 80;
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
      new GuiTextCtrl(PlayStats)
      {
         profile = StatsProfile; position = "180 6"; extent = "150 18"; horizSizing = "right"; visible = true; enabled = true;
         text = "";
      };
   };
   new GuiTextCtrl(PlayNote)
   {
      profile = WorldNoteProfile; position = "100 80"; extent = "600 28"; horizSizing = "center"; visible = true; enabled = true;
      text = "";
   };
};


// ---- the keys

function resetCanvas() { pauseResized(); }

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
// The figure walks (a share of its full speed) and sprints at its full speed while Shift is held.
$World::WalkPace = 0.35;
$World::Pace = $World::WalkPace;
function moveForward(%v) { $World::Forward = %v; updateMove(); }
function moveBack(%v) { $World::Back = %v; updateMove(); }
function moveLeft(%v) { $World::Left = %v; updateMove(); }
function moveRight(%v) { $World::Right = %v; updateMove(); }
function sprint(%v) { $World::Pace = %v ? 1 : $World::WalkPace; updateMove(); }
// Swimming (the host sets $World::Swimming while the figure swims): Space held swims up and Ctrl held dives; Z does nothing.
function updateMove()
{
   $mvForwardAction = $World::Forward * $World::Pace;
   $mvBackwardAction = $World::Back * $World::Pace;
   $mvLeftAction = $World::Left * $World::Pace;
   $mvRightAction = $World::Right * $World::Pace;
   $mvUpAction = $World::Swimming && $World::Up;
   $mvDownAction = $World::Swimming && $Pose::CrouchHeld;
}
// The count goes up on the press and on the release: odd while held, so a quick tap (both in one frame) jumps once.
function jump(%v)
{
   $World::Up = %v != 0;
   updateMove();
   if (!poseJumpKey(%v))
      $mvTriggerCount2++;
}
function diveKey(%v)
{
   crouchKey(%v);
   updateMove();
}
function swimProneKey(%v)
{
   if (!$World::Swimming)
      proneKey(%v);
}
function turn(%v) { $mvYaw += %v * 0.003; }
function look(%v) { $mvPitch += %v * 0.003; }
// Tab: the camera at the figure's eyes or behind it.
function switchCamera(%v)
{
   if (%v)
      setCameraMode(getCameraMode() $= "first-person" ? "third-person" : "first-person");
}
function restart(%v) { if (%v && isObject(ServerConnection)) commandToServer('Respawn'); }
// The left button (the fire binding): the server picks up the prop the figure looks at, or drops the one it holds (its own
// rules: serverCmdPropGrab).
function grab(%v) { if (%v && isObject(ServerConnection)) commandToServer('PropGrab'); }
PlayMap.bind(mouse, button0, grab);
// Q: the menu of a mod the server sends (the game's own key for it: a server's menu binds no keys). The server's script hears the
// command and answers its menu; the host is told the player asked, so the menu may take the cursor again after an Escape.
function catalogKey(%v)
{
   if (%v && isObject(ServerConnection))
   {
      $World::CatalogKey++;
      commandToServer('Catalog');
   }
}
PlayMap.bind(keyboard, q, catalogKey);
PlayMap.bind(keyboard, w, moveForward);
PlayMap.bind(keyboard, s, moveBack);
PlayMap.bind(keyboard, a, moveLeft);
PlayMap.bind(keyboard, d, moveRight);
PlayMap.bind(keyboard, lshift, sprint);
bindPoseKeys(PlayMap);
PlayMap.bind(keyboard, lcontrol, diveKey);
PlayMap.bind(keyboard, z, swimProneKey);
PlayMap.bind(keyboard, space, jump);
PlayMap.bind(keyboard, tab, switchCamera);
PlayMap.bind(keyboard, r, restart);
PlayMap.bind(keyboard, escape, pauseKey);
PlayMap.bind(mouse, xaxis, turn);
PlayMap.bind(mouse, yaxis, look);

// ---- the pause menu's hooks (common/pause.cs) and its list of keys

function pauseAllowed() { return $World::InWorld; }
function onPause()
{
   PlayMap.pop();
   releasePoseKeys();
   $World::Forward = $World::Back = $World::Left = $World::Right = 0;
   $World::Up = false;
   $World::Pace = $World::WalkPace;
   updateMove();
}
function onResume() { PlayMap.push(); }
function leaveGame()
{
   closePause();
   leaveWorld();
}
function quitGame() { quit(); }

addControls(0, "Moving");
addControl("W A S D", "Walk");
addControl("Shift", "Sprint");
addControl("Ctrl", "Crouch while held, or dive");
addControl("C", "Crouch or stand up");
addControl("Z", "Lie down or get up");
addControl("Space", "Jump, or swim up");
addControls(1, "Looking");
addControl("Mouse", "Look around");
addControl("Tab", "Switch camera");
addControls(1, "Game");
addControl("R", "Back to the start");
addControl("Esc", "This menu");

// ---- in and out

function showMenu()
{
   closePause();
   $World::InWorld = 0;
   PlayMap.pop();
   releasePoseKeys();
   $World::Forward = $World::Back = $World::Left = $World::Right = 0;
   $World::Up = false;
   $World::Pace = $World::WalkPace;
   updateMove();
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
   releasePoseKeys();
}

function clientCmdRespawned()
{
   showNote("Back to the start.");
   releasePoseKeys();
}

showMenu();
