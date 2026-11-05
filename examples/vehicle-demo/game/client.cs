// The vehicle demo's client: the menu (play alone, host a game, or join one on the network or at an address), the world's view
// with its help line and hint, the keys on foot and in a seat, and the way in.
//
// The host watches the globals the menu sets: $World::AloneAsked (start a game here that nobody else can join and join it),
// $World::HostAsked (start a server here and join it) and $World::LeaveAsked (the world was left: stop hosting). $World::Port is the UDP port number the games listen on. On foot the keys move the figure (as the level demo's); a click asks
// the server for a seat (serverCmdGetIn). In a seat ($Drive::Seat: 0 the driver's, 1 the passenger's, -1 on foot) the keys
// set $Drive::Throttle and $Drive::Steer, which the host turns into the moves that drive the buggy each step (the driver's
// only), Space is the brake (the jump trigger, the buggy's for a driver), and the mouse turns $Drive::OrbitYaw and
// $Drive::OrbitPitch, the camera's place round the buggy (in first person, the head's turn). Tab ($World::FirstPerson) puts the
// camera at the figure's eyes or behind it, on foot and in a seat. The host writes the hint line (setHint).

exec("common/client.cs");

// ---- profiles

new GuiControlProfile(WorldBackProfile) { opaque = true; fillColor = "18 22 30 255"; };
new GuiControlProfile(WorldBandProfile) { opaque = true; fillColor = "12 16 24 160"; };
new GuiControlProfile(WorldTitleProfile) { fontType = "Arial"; fontSize = 44; fontColors[0] = "240 242 248 255"; };
new GuiControlProfile(WorldTextProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "222 226 236 255"; };
new GuiControlProfile(WorldLabelProfile) { fontType = "Arial"; fontSize = 18; fontColors[0] = "222 226 236 255"; };
new GuiControlProfile(WorldDimProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "150 160 178 255"; };
new GuiControlProfile(WorldNoteProfile) { fontType = "Arial"; fontSize = 22; fontColors[0] = "255 214 110 255"; justify = "center"; };
new GuiControlProfile(WorldHintProfile) { fontType = "Arial"; fontSize = 24; fontColors[0] = "255 255 255 255"; justify = "center"; };
new GuiControlProfile(WorldHintBackProfile) { opaque = true; fillColor = "12 16 24 150"; };
new GuiControlProfile(WorldScrollProfile) { opaque = true; border = true; fillColor = "16 20 28 255"; borderColor = "70 82 104 255"; };
new GuiControlProfile(WorldListProfile)
{
   fontType = "Arial";
   fontSize = 18;
   fontColors[0] = "226 230 238 255";
   fontColors[1] = "255 214 110 255";
   fontColorHL = "255 214 110 255";
   fillColorHL = "58 84 128 255";
   opaque = false;
   canKeyFocus = true;
};
new GuiControlProfile(WorldEditProfile)
{
   opaque = true;
   border = true;
   fillColor = "16 20 28 230";
   fillColorHL = "90 110 150 255";
   borderColor = "255 190 90 255";
   fontType = "Arial";
   fontSize = 18;
   fontColors[0] = "240 242 248 255";
   fontColorHL = "255 255 255 255";
   cursorColor = "255 214 110 255";
   canKeyFocus = true;
   autoSizeHeight = false;
};
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

createCanvas("torque-rs vehicle demo");

// ---- the menu

new GuiControl(MenuGui)
{
   profile = WorldBackProfile; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(MenuPanel)
   {
      position = "50 20"; extent = "700 560"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(MenuTitle)
      {
         profile = WorldTitleProfile; position = "0 0"; extent = "700 52"; visible = true; enabled = true;
         text = "Vehicle Demo";
      };
      new GuiTextCtrl(MenuLine)
      {
         profile = WorldDimProfile; position = "2 56"; extent = "700 20"; visible = true; enabled = true;
         text = "Drive a buggy over the hills, alone or with a friend on the same network.";
      };
      new GuiTextCtrl(NameLabel)
      {
         profile = WorldLabelProfile; position = "2 102"; extent = "110 24"; visible = true; enabled = true; text = "Your name";
      };
      new GuiTextEditCtrl(NameEdit)
      {
         profile = WorldEditProfile; position = "120 98"; extent = "300 32"; visible = true; enabled = true; maxLength = 24;
         text = "";
      };
      new GuiButtonCtrl(AloneButton)
      {
         profile = WorldButtonProfile; position = "0 150"; extent = "260 50"; visible = true; enabled = true;
         text = "Play alone"; command = "playAlone();";
      };
      new GuiButtonCtrl(HostButton)
      {
         profile = WorldButtonProfile; position = "0 210"; extent = "260 50"; visible = true; enabled = true;
         text = "Host a game"; command = "hostGame();";
      };
      new GuiTextCtrl(GamesLabel)
      {
         profile = WorldLabelProfile; position = "2 284"; extent = "400 24"; visible = true; enabled = true;
         text = "Games on your network";
      };
      new GuiScrollCtrl(GamesScroll)
      {
         profile = WorldScrollProfile; position = "0 312"; extent = "700 100"; visible = true; enabled = true;
         hScrollBar = "alwaysOff"; vScrollBar = "dynamic";
         new GuiTextListCtrl(GamesList)
         {
            profile = WorldListProfile; position = "2 2"; extent = "680 20"; visible = true; enabled = true; columns = "0 480";
         };
      };
      new GuiButtonCtrl(RefreshButton)
      {
         profile = WorldButtonProfile; position = "0 422"; extent = "200 40"; visible = true; enabled = true;
         text = "Refresh"; command = "refreshGames();";
      };
      new GuiButtonCtrl(JoinSelectedButton)
      {
         profile = WorldButtonProfile; position = "500 422"; extent = "200 40"; visible = true; enabled = true;
         text = "Join"; command = "joinSelected();";
      };
      new GuiTextCtrl(AddressLabel)
      {
         profile = WorldLabelProfile; position = "2 482"; extent = "80 24"; visible = true; enabled = true; text = "Address";
      };
      new GuiTextEditCtrl(AddressEdit)
      {
         profile = WorldEditProfile; position = "90 478"; extent = "400 32"; visible = true; enabled = true; maxLength = 260;
         text = ""; placeholderText = "127.0.0.1:28000"; altCommand = "joinAddress();";
      };
      new GuiButtonCtrl(JoinAddressButton)
      {
         profile = WorldButtonProfile; position = "500 474"; extent = "200 40"; visible = true; enabled = true;
         text = "Join address"; command = "joinAddress();";
      };
      new GuiTextCtrl(MenuStatus)
      {
         profile = WorldNoteProfile; position = "0 526"; extent = "700 28"; visible = true; enabled = true; text = "";
      };
   };
};

function saveName()
{
   %name = trim(NameEdit.getValue());
   if (%name !$= "")
      $Pref::Player::Name = %name;
   NameEdit.setValue($Pref::Player::Name);
}

function playAlone()
{
   saveName();
   MenuStatus.setText("Starting a game...");
   $World::AloneAsked = 1;
}

function hostGame()
{
   saveName();
   cancelJoinLookup();
   MenuStatus.setText("Starting a game...");
   $World::HostAsked = 1;
}

function refreshGames()
{
   GamesList.clear();
   MenuStatus.setText("Looking for games...");
   queryLanServers($World::Port, 0, "", "", 0, 100, 0, 2, 0, 100, 0);
}

function onServerQueryStatus(%status, %message, %value)
{
   if (%status !$= "done")
      return;
   GamesList.clear();
   for (%i = 0; %i < getServerCount(); %i++)
   {
      if (!setServerInfo(%i))
         continue;
      GamesList.addRow(%i, $ServerInfo::Name TAB $ServerInfo::PlayerCount @ " playing");
   }
   if (GamesList.rowCount() > 0)
   {
      GamesList.setSelectedRow(0);
      MenuStatus.setText("Pick a game and press Join.");
   }
   else
      MenuStatus.setText("No games found. Host one, or type an address.");
}

function joinSelected()
{
   saveName();
   %row = GamesList.getSelectedId();
   if (%row $= "" || %row < 0 || !setServerInfo(%row))
   {
      MenuStatus.setText("Pick a game first.");
      return;
   }
   cancelJoinLookup();
   MenuStatus.setText("Joining " @ $ServerInfo::Name @ "...");
   connectTo($ServerInfo::Address);
}

// Joins the address or name typed in the box (with :port, or without: port 28000); a name is looked up first.
function joinAddress()
{
   saveName();
   %address = checkJoinAddress(trim(AddressEdit.getValue()));
   if (getSubStr(%address, 0, 1) $= "!")
   {
      MenuStatus.setText(getSubStr(%address, 1, 200));
      return;
   }
   if (getSubStr(%address, 0, 1) $= "?")
   {
      MenuStatus.setText("Looking up " @ getSubStr(%address, 1, 260) @ "...");
      schedule(50, 0, pollJoinLookup);
      return;
   }
   MenuStatus.setText("Connecting to " @ %address @ "...");
   armPublicJoin(%address);
   connectTo(%address);
}

// Connects once the typed name's lookup ends; leaving the menu drops it.
function pollJoinLookup()
{
   if (Canvas.getContent() != MenuGui.getId())
   {
      cancelJoinLookup();
      return;
   }
   %address = joinLookupResult();
   if (%address $= "?")
      schedule(50, 0, pollJoinLookup);
   else if (getSubStr(%address, 0, 1) $= "!")
      MenuStatus.setText(getSubStr(%address, 1, 200));
   else if (%address !$= "")
   {
      MenuStatus.setText("Connecting to " @ %address @ "...");
      connectTo(%address);
   }
}

// ---- the world's view

$World::FootHelp = "W A S D move   Shift walk   Space jump   Mouse look   Click the buggy to get in   Tab camera   Wheel zoom   [ ] time   R restart   Esc menu";
$World::DriveHelp = "W throttle   S reverse   A D steer   Space brake   F light   Mouse turns the camera   Tab camera   Wheel zoom   [ ] time   E get out   R restart   Esc menu";
$World::RideHelp = "Mouse looks around   Tab camera   Wheel zoom   [ ] time   E get out   R restart   Esc menu";

new GameTSCtrl(PlayGui)
{
   position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(PlayBand)
   {
      profile = WorldBandProfile; position = "0 0"; extent = "800 28"; horizSizing = "width";
      new GuiTextCtrl(PlayTitle)
      {
         profile = WorldTextProfile; position = "10 5"; extent = "120 18"; visible = true; enabled = true; text = "";
      };
      new GuiTextCtrl(PlayHelp)
      {
         profile = WorldDimProfile; position = "130 5"; extent = "660 18"; horizSizing = "width"; visible = true; enabled = true;
         text = $World::FootHelp;
      };
   };
   new GuiTextCtrl(PlayNote)
   {
      profile = WorldNoteProfile; position = "100 80"; extent = "600 28"; horizSizing = "center"; visible = true; enabled = true;
      text = "";
   };
   new GuiControl(HintBack)
   {
      profile = WorldHintBackProfile; position = "220 530"; extent = "360 40"; horizSizing = "center"; vertSizing = "top";
      visible = false;
      new GuiTextCtrl(HintText)
      {
         profile = WorldHintProfile; position = "0 7"; extent = "360 28"; horizSizing = "width"; visible = true; enabled = true;
         text = "";
      };
   };
};

// The hint line at the bottom of the view (none: hidden).
function setHint(%text)
{
   HintText.setText(%text);
   HintBack.setVisible(%text !$= "");
}

// ---- the keys on foot

function resetCanvas() {}

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
// The jump trigger is held while the key is (an odd count).
function jump(%v) { $mvTriggerCount2++; }
function turn(%v) { $mvYaw += %v * 0.003; }
function look(%v) { $mvPitch += %v * 0.003; }
// Tab: the camera at the figure's eyes or behind it (in a seat: behind the buggy).
function switchCamera(%v) { if (%v) $World::FirstPerson = !$World::FirstPerson; }
function restart(%v) { if (%v && isObject(ServerConnection)) commandToServer('Respawn'); }
function getIn(%v) { if (%v && isObject(ServerConnection)) commandToServer('GetIn'); }
function menuKey(%v) { if (%v) leaveWorld(); }
// The wheel: the camera behind in (away from you) or out; the client takes the notches each frame.
function zoomCamera(%v) { $World::Zoom += %v; }
// ] and [: the day faster or slower, \ back to its own pace (the server's day: only the host may).
function dayFaster(%v) { if (%v && isObject(ServerConnection)) commandToServer('DaySpeed', 1); }
function daySlower(%v) { if (%v && isObject(ServerConnection)) commandToServer('DaySpeed', -1); }
function dayNormal(%v) { if (%v && isObject(ServerConnection)) commandToServer('DaySpeed', 0); }
PlayMap.bind(keyboard, w, moveForward);
PlayMap.bind(keyboard, s, moveBack);
PlayMap.bind(keyboard, a, moveLeft);
PlayMap.bind(keyboard, d, moveRight);
PlayMap.bind(keyboard, lshift, walkSlowly);
PlayMap.bind(keyboard, space, jump);
PlayMap.bind(keyboard, tab, switchCamera);
PlayMap.bind(keyboard, r, restart);
PlayMap.bind(keyboard, escape, menuKey);
PlayMap.bind(mouse, xaxis, turn);
PlayMap.bind(mouse, yaxis, look);
PlayMap.bind(mouse, button0, getIn);
PlayMap.bind(mouse, zaxis, zoomCamera);
PlayMap.bind(keyboard, rbracket, dayFaster);
PlayMap.bind(keyboard, lbracket, daySlower);
PlayMap.bind(keyboard, backslash, dayNormal);

// ---- the keys in a seat

new ActionMap(DriveMap);
function driveForward(%v) { $Drive::Forward = %v; $Drive::Throttle = $Drive::Forward - $Drive::Back; }
function driveBack(%v) { $Drive::Back = %v; $Drive::Throttle = $Drive::Forward - $Drive::Back; }
function steerLeft(%v) { $Drive::Left = %v; $Drive::Steer = $Drive::Left - $Drive::Right; }
function steerRight(%v) { $Drive::Right = %v; $Drive::Steer = $Drive::Left - $Drive::Right; }
// The brake: the jump trigger held while the key is (a driver's goes to the buggy).
function brake(%v) { $Drive::Brake = %v; $mvTriggerCount2++; }
function orbitYaw(%v) { $Drive::OrbitYaw += %v * 0.004; }
// (mClamp is an integer's: the pitch is kept by getMin and getMax.)
function orbitPitch(%v) { $Drive::OrbitPitch = getMax(-1.5, getMin($Drive::OrbitPitch + %v * 0.003, 0.9)); }
function getOut(%v) { if (%v && isObject(ServerConnection)) commandToServer('GetOut'); }
// F: the headlight on or off (the server lets only the driver).
function toggleLight(%v) { if (%v && isObject(ServerConnection)) commandToServer('ToggleLight'); }
DriveMap.bind(keyboard, w, driveForward);
DriveMap.bind(keyboard, s, driveBack);
DriveMap.bind(keyboard, a, steerLeft);
DriveMap.bind(keyboard, d, steerRight);
DriveMap.bind(keyboard, space, brake);
DriveMap.bind(keyboard, e, getOut);
DriveMap.bind(keyboard, tab, switchCamera);
DriveMap.bind(keyboard, f, toggleLight);
DriveMap.bind(keyboard, escape, menuKey);
DriveMap.bind(mouse, xaxis, orbitYaw);
DriveMap.bind(mouse, yaxis, orbitPitch);
DriveMap.bind(mouse, zaxis, zoomCamera);
DriveMap.bind(keyboard, r, restart);
DriveMap.bind(keyboard, rbracket, dayFaster);
DriveMap.bind(keyboard, lbracket, daySlower);
DriveMap.bind(keyboard, backslash, dayNormal);

// Lets go of every key of both maps (a map change leaves no key held).
function releaseKeys()
{
   $World::Forward = $World::Back = $World::Left = $World::Right = 0;
   updateMove();
   $Drive::Forward = $Drive::Back = $Drive::Left = $Drive::Right = 0;
   $Drive::Throttle = $Drive::Steer = 0;
   $Drive::Brake = 0;
   if ($mvTriggerCount2 % 2)
      $mvTriggerCount2++;
}

// ---- in and out

function showMenu()
{
   $World::InWorld = 0;
   $Drive::Seat = -1;
   PlayMap.pop();
   DriveMap.pop();
   NameEdit.setValue($Pref::Player::Name);
   Canvas.setContent(MenuGui);
   Canvas.cursorOn();
}

function leaveWorld()
{
   disconnect();
   releaseKeys();
   MenuStatus.setText("");
   showMenu();
   $World::LeaveAsked = 1;
}

function onDataBlockObjectReceived(%index, %total)
{
   MenuStatus.setText("Loading the world... " @ %index + 1 @ " of " @ %total);
}

function onGhostAlwaysStarted(%count) {}
function onGhostAlwaysObjectReceived() {}

function GameConnection::onConnectionAccepted(%this)
{
   MenuStatus.setText("Joined. Waiting for the world...");
}

function GameConnection::onConnectRequestRejected(%this, %reason)
{
   MenuStatus.setText("The game turned us away: " @ %reason);
}

function GameConnection::onConnectRequestTimedOut(%this)
{
   MenuStatus.setText("The game did not answer.");
}

function GameConnection::onConnectionDropped(%this, %reason)
{
   leaveWorld();
   MenuStatus.setText("Left the game: " @ %reason);
}

function GameConnection::onConnectionTimedOut(%this)
{
   leaveWorld();
   MenuStatus.setText("Lost the connection to the game.");
}

function clientCmdWorldStarted(%title)
{
   PlayTitle.setText(%title);
   DriveMap.pop();
   PlayMap.push();
   Canvas.setContent(PlayGui);
   Canvas.cursorOff();
   $World::InWorld = 1;
   $Drive::Seat = -1;
   showNote(%title);
}

// The server put the figure in seat %seat (0 the driver's, 1 the passenger's) or back on its feet (-1).
function clientCmdSeated(%seat)
{
   releaseKeys();
   // The mouse's turn on foot goes nowhere: it would steer.
   $mvYaw = 0;
   $mvPitch = 0;
   $Drive::Seat = %seat;
   $Drive::OrbitYaw = 0;
   $Drive::OrbitPitch = 0;
   if (%seat >= 0)
   {
      PlayMap.pop();
      DriveMap.push();
      PlayHelp.setText(%seat == 0 ? $World::DriveHelp : $World::RideHelp);
   }
   else
   {
      DriveMap.pop();
      PlayMap.push();
      PlayHelp.setText($World::FootHelp);
   }
}

function clientCmdFell()
{
   showNote("You fell off. Back to the start.");
}

function clientCmdRespawned()
{
   showNote("Back to the start.");
}

// The day's pace changed (%speed times its own, 0 stopped) at %time of the day (0 sunrise, 0.25 noon), or (%speed "") it may
// not: the hint line says so for a moment.
function clientCmdDaySpeed(%speed, %time, %why)
{
   if (%why !$= "")
      setDayNote(%why);
   else
      setDayNote("Time" SPC dayClock(%time) @ "," SPC (%speed == 0 ? "stopped" : %speed @ "x"));
}

// The clock at %time of the day (sunrise at 6:00).
function dayClock(%time)
{
   %minutes = mFloor((%time - mFloor(%time)) * 1440 + 360) % 1440;
   %h = mFloor(%minutes / 60);
   %m = %minutes % 60;
   return %h @ ":" @ (%m < 10 ? "0" : "") @ %m;
}

function setDayNote(%text)
{
   $World::DayNote = %text;
   cancel($World::DayNoteTimer);
   $World::DayNoteTimer = schedule(2500, 0, "clearDayNote");
}

function clearDayNote() { $World::DayNote = ""; }

showMenu();
