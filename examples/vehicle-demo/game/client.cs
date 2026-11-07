// The vehicle demo's client: the menu (play alone, host a game, or join one on the network or at an address), the world's view
// with its title, readout and hint, the keys on foot and in a seat, and the way in.
//
// The host watches the globals the menu sets: $World::AloneAsked (start a game here that nobody else can join and join it),
// $World::HostAsked (start a server here and join it) and $World::LeaveAsked (the world was left: stop hosting). $World::Port is the UDP port number the games listen on; $World::AloneOnly (set by the host) keeps the menu to playing alone, and with $World::WebRooms (a browser's page not played alone) to the page's own hosting and joining by room code. On foot the keys move the figure (as the level demo's); a click asks
// the server for a seat (serverCmdGetIn), which points the figure's arm where it looks when the click meets no free seat. In a seat ($Drive::Seat: 0 the driver's, 1 the passenger's, -1 on foot) the keys
// set $Drive::Throttle and $Drive::Steer, which the host turns into the moves that drive the buggy each step (the driver's
// only), Space is the brake (the jump trigger, the buggy's for a driver), and the mouse turns $Drive::OrbitYaw and
// $Drive::OrbitPitch, the camera's place round the buggy (in first person, the head's turn). Tab ($World::FirstPerson) puts the
// camera at the figure's eyes or behind it, on foot and in a seat. Esc opens the examples' pause menu (common/pause.cs: Resume,
// Controls, Leave game, Quit) and T the chat (common/chat.cs). The host writes the hint line (setHint).

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
exec("common/pause.cs");

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
      new GuiTextCtrl(PlayStats)
      {
         profile = StatsProfile; position = "640 6"; extent = "150 18"; horizSizing = "left"; visible = true; enabled = true;
         text = "";
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

// The chat (common/chat.cs): its box at the view's bottom left, over the hint line's height.
exec("common/chat.cs");
makeChat(PlayGui, 12, 354, "top");

// The hint line at the bottom of the view (none: hidden).
function setHint(%text)
{
   HintText.setText(%text);
   HintBack.setVisible(%text !$= "");
}

// ---- the keys on foot

function resetCanvas() { pauseResized(); }

new ActionMap(PlayMap);
// The figure walks (a share of its full speed) and sprints at its full speed while Shift is held.
$World::WalkPace = 0.35;
$World::Pace = $World::WalkPace;
function moveForward(%v) { $World::Forward = %v; updateMove(); }
function moveBack(%v) { $World::Back = %v; updateMove(); }
function moveLeft(%v) { $World::Left = %v; updateMove(); }
function moveRight(%v) { $World::Right = %v; updateMove(); }
function sprint(%v) { $World::Pace = %v ? 1 : $World::WalkPace; updateMove(); }
function updateMove()
{
   $mvForwardAction = $World::Forward * $World::Pace;
   $mvBackwardAction = $World::Back * $World::Pace;
   $mvLeftAction = $World::Left * $World::Pace;
   $mvRightAction = $World::Right * $World::Pace;
}
// The jump trigger is held while the key is (an odd count).
function jump(%v) { if (!poseJumpKey(%v)) $mvTriggerCount2++; }
function turn(%v) { $mvYaw += %v * 0.003; }
function look(%v) { $mvPitch += %v * 0.003; }
// Tab: the camera at the figure's eyes or behind it (in a seat: behind the buggy).
function switchCamera(%v) { if (%v) $World::FirstPerson = !$World::FirstPerson; }
function restart(%v) { if (%v && isObject(ServerConnection)) commandToServer('Respawn'); }
// The left button: a click on a buggy takes a seat in it, one on anything else points where the figure looks while the button
// is held (the server holds it a second at least).
function getIn(%v)
{
   if (!isObject(ServerConnection))
      return;
   if (%v)
      commandToServer('GetIn');
   else
      commandToServer('PointDone');
}
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
PlayMap.bind(keyboard, lshift, sprint);
bindPoseKeys(PlayMap);
PlayMap.bind(keyboard, space, jump);
PlayMap.bind(keyboard, tab, switchCamera);
PlayMap.bind(keyboard, r, restart);
PlayMap.bind(keyboard, t, chatKey);
PlayMap.bind(keyboard, escape, pauseKey);
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
// Each move is kept in the range the host gives the camera ($Drive::YawMin to $Drive::YawMax, $Drive::PitchMin to
// $Drive::PitchMax), so a mouse pushed past the edge turns the view back at once. (mClamp is an integer's: getMin and getMax.)
function orbitYaw(%v) { $Drive::OrbitYaw = getMax($Drive::YawMin, getMin($Drive::OrbitYaw + %v * 0.004, $Drive::YawMax)); }
function orbitPitch(%v) { $Drive::OrbitPitch = getMax($Drive::PitchMin, getMin($Drive::OrbitPitch + %v * 0.003, $Drive::PitchMax)); }
$Drive::YawMin = -1000000;
$Drive::YawMax = 1000000;
$Drive::PitchMin = -0.8;
$Drive::PitchMax = 1.0;
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
DriveMap.bind(keyboard, t, chatKey);
DriveMap.bind(keyboard, escape, pauseKey);
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
   releaseMoveKeys();
   releasePoseKeys();
}

// Lets go of every key of both maps but the poses C and Z chose.
function releaseMoveKeys()
{
   $World::Forward = $World::Back = $World::Left = $World::Right = 0;
   $World::Pace = $World::WalkPace;
   updateMove();
   $Drive::Forward = $Drive::Back = $Drive::Left = $Drive::Right = 0;
   $Drive::Throttle = $Drive::Steer = 0;
   $Drive::Brake = 0;
   if ($mvTriggerCount2 % 2)
      $mvTriggerCount2++;
   // A held button's point ends as if it came up.
   if (isObject(ServerConnection))
      commandToServer('PointDone');
}

// ---- in and out

// Gives `%panel` the size `%w` by `%h`, centered in `%parent` (its horizSizing and vertSizing "center" keep it there as the
// window changes): a menu with fewer controls shows as a narrower column in the middle.
function fitPanel(%panel, %parent, %w, %h)
{
   %size = %parent.getExtent();
   %panel.resize(mFloor((getWord(%size, 0) - %w) / 2), mFloor((getWord(%size, 1) - %h) / 2), %w, %h);
}

function showMenu()
{
   $World::InWorld = 0;
   $Drive::Seat = -1;
   PlayMap.pop();
   DriveMap.pop();
   NameEdit.setValue($Pref::Player::Name);
   Canvas.setContent(MenuGui);
   Canvas.cursorOn();
   if ($World::AloneOnly)
      showAloneOnly();
}

// A client with no socket of its own ($World::AloneOnly, a browser's) only plays alone: the menu offers that and nothing else,
// in a column the panel fits, centered.
function showAloneOnly()
{
   fitPanel(MenuPanel, MenuGui, 480, 370);
   MenuTitle.resize(0, 0, 480, 52);
   MenuLine.resize(2, 56, 478, 20);
   MenuStatus.resize(0, 338, 480, 28);
   MenuLine.setText("Drive a buggy over the hills.");
   HostButton.setVisible(0);
   GamesLabel.setVisible(0);
   GamesScroll.setVisible(0);
   RefreshButton.setVisible(0);
   JoinSelectedButton.setVisible(0);
   AddressLabel.setVisible(0);
   AddressEdit.setVisible(0);
   JoinAddressButton.setVisible(0);
   if ($World::WebRooms)
      showWebRooms();
   if ($World::WebInvites)
      showWebInvites();
}

// A browser's page played alone ($World::WebInvites, set by the host): the menu also offers to host with an invite link
// and to join with one (the host also sets $Pause::CanInvite: the Esc menu's Invite players); each asks the page for its
// panel ($World::PageAsk).
function showWebInvites()
{
   MenuLine.setText("Drive a buggy over the hills, alone or with friends you send a link.");
   HostButton.setVisible(1);
   HostButton.setText("Host with a link");
   HostButton.command = "invitePlayers();";
   JoinAddressButton.resize(0, 270, 260, 50);
   JoinAddressButton.setText("Join with Link");
   JoinAddressButton.command = "$World::PageAsk = \"join\";";
   JoinAddressButton.setVisible(1);
}

// The menu's Host with a link and the Esc menu's Invite players: the page's invite panel.
function invitePlayers()
{
   saveName();
   $World::PageAsk = "host";
}

// A browser's page that is not one played alone ($World::WebRooms, set by the host): the menu also offers to host a game
// others join by its room's code ($World::WebHostAsked) and to join one by a code typed in ($World::WebJoinCode); the host
// switches the page's game in place for either. A hosted game's code ($World::RoomCode) shows on the menu and the world's
// title.
function showWebRooms()
{
   MenuLine.setText("Drive a buggy over the hills, alone or with friends who have your room's code.");
   HostButton.setVisible(1);
   HostButton.command = "hostOnPage();";
   AddressLabel.setText("Room");
   AddressLabel.setVisible(1);
   AddressEdit.altCommand = "joinRoomCode();";
   AddressEdit.setVisible(1);
   JoinAddressButton.setText("Join room");
   JoinAddressButton.command = "joinRoomCode();";
   JoinAddressButton.setVisible(1);
   if ($World::RoomCode !$= "")
      MenuStatus.setText("Others join this game with the room code " @ $World::RoomCode @ ".");
}

function hostOnPage()
{
   saveName();
   MenuStatus.setText("Hosting a game...");
   $World::WebHostAsked = 1;
}

function joinRoomCode()
{
   saveName();
   %code = trim(AddressEdit.getValue());
   if (%code $= "")
   {
      MenuStatus.setText("Type the room's code first (like ABCD-EFGH).");
      return;
   }
   MenuStatus.setText("Joining room " @ %code @ "...");
   $World::WebJoinCode = %code;
}

// What the page says on the menu's status line (a game it could not host): `%text` is shown as it is.
function showMenuStatus(%text)
{
   MenuStatus.setText(%text);
}

// The hosted game's room code came ($World::RoomCode): on the world's title.
function showRoomCode()
{
   if ($World::InWorld)
      PlayTitle.setText($World::TitleText @ "   Room " @ $World::RoomCode);
}

function leaveWorld()
{
   clearChat();
   closePause();
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
   closePause();
   $World::TitleText = %title;
   PlayTitle.setText(%title @ ($World::RoomCode $= "" ? "" : "   Room " @ $World::RoomCode));
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
   // While the pause menu or the chat is up the keys stay off: closing it brings back the map of the seat or of the feet.
   if (!$Client::Paused && !$Chat::Open)
      pushPlayKeys();
}

// The keys of where the figure is: on foot or in a seat.
function pushPlayKeys()
{
   PlayMap.pop();
   DriveMap.pop();
   // In a seat the figure sits: no pose key stays held.
   if ($Drive::Seat >= 0)
   {
      releasePoseKeys();
      DriveMap.push();
   }
   else
      PlayMap.push();
}

// ---- the pause menu's hooks (common/pause.cs)

function pauseAllowed() { return $World::InWorld; }
function onPause()
{
   PlayMap.pop();
   DriveMap.pop();
   releaseKeys();
}
function onResume() { pushPlayKeys(); }
// The chat's: the keys off the figure or the buggy while it is open (a pose C or Z chose stays).
function onChatOpen()
{
   PlayMap.pop();
   DriveMap.pop();
   releaseMoveKeys();
   releaseHeldPoseKeys();
}
function onChatClose() { pushPlayKeys(); }
function leaveGame()
{
   closePause();
   leaveWorld();
}
function quitGame() { quit(); }

addControls(0, "On foot");
addControl("W A S D", "Walk");
addControl("Shift", "Sprint");
addControl("Ctrl", "Crouch while held");
addControl("C", "Crouch or stand up");
addControl("Z", "Lie down or get up");
addControl("Space", "Jump");
addControl("Mouse", "Look around");
addControl("Click the buggy", "Get in");
addControls(0, "In the passenger seat");
addControl("Mouse", "Look around");
addControl("E", "Get out");
addControls(1, "At the wheel");
addControl("W", "Throttle");
addControl("S", "Reverse");
addControl("A D", "Steer");
addControl("Space", "Brake");
addControl("F", "Headlight on or off");
addControl("Mouse", "Turn the camera");
addControl("E", "Get out");
addControls(1, "Any time");
addControl("Tab", "Switch camera");
addControl("Mouse wheel", "Zoom the camera");
addControl("[ and ]", "Day slower or faster");
addControl("\\", "Day at normal speed");
addControl("R", "Back to the start");
addControl("T", "Chat");
addControl("Esc", "This menu");

function clientCmdFell()
{
   showNote("You fell off. Back to the start.");
   releasePoseKeys();
}

function clientCmdRespawned()
{
   showNote("Back to the start.");
   releasePoseKeys();
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
