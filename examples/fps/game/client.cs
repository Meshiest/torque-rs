// The arena's client: the main menu, the list of games on the local network, the view with its HUD (rounds, health, chat),
// the keys, and the way in and out.
//
// The host watches three globals the scripts set: $Arena::HostAsked (start a server here and join it), $Arena::LeaveAsked (the
// view was left: stop hosting) and $Arena::QuitAsked. $Arena::Port is the UDP port number the games listen on.

// ---- profiles

new GuiControlProfile(ArenaBackProfile) { opaque = true; fillColor = "20 24 32 255"; };
new GuiControlProfile(ArenaPanelProfile) { opaque = true; border = true; fillColor = "30 36 48 235"; borderColor = "70 82 104 255"; };
new GuiControlProfile(ArenaBandProfile) { opaque = true; fillColor = "10 12 18 150"; };
new GuiControlProfile(ArenaTitleProfile) { fontType = "Arial"; fontSize = 48; fontColors[0] = "246 240 228 255"; };
new GuiControlProfile(ArenaTextProfile) { fontType = "Arial"; fontSize = 18; fontColors[0] = "226 230 238 255"; };
new GuiControlProfile(ArenaDimProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "150 160 178 255"; };
new GuiControlProfile(ArenaNoteProfile) { fontType = "Arial"; fontSize = 24; fontColors[0] = "255 208 120 255"; justify = "center"; };
new GuiControlProfile(ArenaBigProfile) { fontType = "Arial"; fontSize = 40; fontColors[0] = "255 236 200 255"; justify = "right"; };
new GuiControlProfile(ArenaHealthProfile) { fontType = "Arial"; fontSize = 40; fontColors[0] = "170 232 170 255"; };
new GuiControlProfile(ArenaCrossProfile) { fontType = "Arial"; fontSize = 28; fontColors[0] = "255 255 255 220"; justify = "center"; };
new GuiControlProfile(ArenaChatProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "236 240 248 255"; };
new GuiControlProfile(ArenaInfoProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "255 208 120 255"; };
new GuiControlProfile(ArenaListProfile)
{
   fontType = "Arial";
   fontSize = 18;
   fontColors[0] = "226 230 238 255";
   // The selected row: the normal light text on a muted blue; a row under the mouse: warm text on the dark list.
   fontColors[1] = "255 208 120 255";
   fontColorHL = "255 208 120 255";
   fillColorHL = "58 84 128 255";
   opaque = false;
   canKeyFocus = true;
};
new GuiControlProfile(ArenaScrollProfile) { opaque = true; border = true; fillColor = "16 20 28 255"; borderColor = "70 82 104 255"; };
new GuiControlProfile(ArenaEditProfile)
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
   cursorColor = "255 208 120 255";
   canKeyFocus = true;
   autoSizeHeight = false;
};
new GuiControlProfile(ArenaButtonProfile)
{
   opaque = true;
   border = true;
   fillColor = "48 58 78 255";
   fillColorHL = "255 176 72 255";
   fillColorNA = "30 34 42 255";
   borderColor = "100 118 150 255";
   fontType = "Arial";
   fontSize = 22;
   fontColors[0] = "238 242 248 255";
   fontColors[1] = "20 24 32 255";
   fontColors[2] = "120 126 138 255";
   justify = "center";
   canKeyFocus = true;
};

createCanvas("torque-rs arena");

// ---- the main menu

new GuiControl(MainMenuGui)
{
   profile = ArenaBackProfile; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiTextCtrl(MainTitle)
   {
      profile = ArenaTitleProfile; position = "70 70"; extent = "660 56"; visible = true; enabled = true;
      text = "Arena";
   };
   new GuiTextCtrl(MainLine)
   {
      profile = ArenaDimProfile; position = "72 130"; extent = "660 20"; visible = true; enabled = true;
      text = "A small shooter for one player or a few on the same network.";
   };
   new GuiTextCtrl(NameLabel)
   {
      profile = ArenaTextProfile; position = "72 186"; extent = "120 24"; visible = true; enabled = true;
      text = "Your name";
   };
   new GuiTextEditCtrl(NameEdit)
   {
      profile = ArenaEditProfile; position = "190 182"; extent = "240 30"; visible = true; enabled = true;
      maxLength = 24; text = "";
      altCommand = "saveName();";
   };
   new GuiButtonCtrl(HostButton)
   {
      profile = ArenaButtonProfile; position = "70 250"; extent = "280 56"; visible = true; enabled = true;
      text = "Host game"; command = "hostGame();";
   };
   new GuiButtonCtrl(JoinButton)
   {
      profile = ArenaButtonProfile; position = "70 322"; extent = "280 56"; visible = true; enabled = true;
      text = "Join game"; command = "showJoin();";
   };
   new GuiButtonCtrl(QuitButton)
   {
      profile = ArenaButtonProfile; position = "70 394"; extent = "280 56"; visible = true; enabled = true;
      text = "Quit"; command = "quitGame();";
   };
   new GuiTextCtrl(HostNote)
   {
      profile = ArenaDimProfile; position = "372 268"; extent = "380 20"; visible = true; enabled = true;
      text = "Start a game here. Others on your network can join it.";
   };
   new GuiTextCtrl(JoinNote)
   {
      profile = ArenaDimProfile; position = "372 340"; extent = "380 20"; visible = true; enabled = true;
      text = "Find a game someone is hosting on your network.";
   };
   new GuiTextCtrl(MainStatus)
   {
      profile = ArenaNoteProfile; position = "70 520"; extent = "660 28"; vertSizing = "top"; visible = true; enabled = true;
      text = "";
   };
};

function saveName()
{
   %name = trim(NameEdit.getValue());
   if (%name !$= "")
      $Pref::Player::Name = %name;
   NameEdit.setValue($Pref::Player::Name);
}

function hostGame()
{
   saveName();
   MainStatus.setText("Starting a game...");
   $Arena::HostAsked = 1;
}

function quitGame()
{
   $Arena::QuitAsked = 1;
   quit();
}

// ---- the list of games on the network

new GuiControl(JoinGui)
{
   profile = ArenaBackProfile; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiTextCtrl(JoinTitle)
   {
      profile = ArenaTitleProfile; position = "70 50"; extent = "660 56"; visible = true; enabled = true;
      text = "Join a game";
   };
   new GuiTextCtrl(JoinHeaderGame)
   {
      profile = ArenaDimProfile; position = "74 124"; extent = "200 20"; visible = true; enabled = true; text = "Game";
   };
   new GuiTextCtrl(JoinHeaderPlayers)
   {
      profile = ArenaDimProfile; position = "374 124"; extent = "90 20"; visible = true; enabled = true; text = "Players";
   };
   new GuiTextCtrl(JoinHeaderPing)
   {
      profile = ArenaDimProfile; position = "474 124"; extent = "80 20"; visible = true; enabled = true; text = "Ping";
   };
   new GuiTextCtrl(JoinHeaderMap)
   {
      profile = ArenaDimProfile; position = "564 124"; extent = "160 20"; visible = true; enabled = true; text = "Map";
   };
   new GuiScrollCtrl(ServerScroll)
   {
      profile = ArenaScrollProfile; position = "70 148"; extent = "660 300"; visible = true; enabled = true;
      hScrollBar = "alwaysOff"; vScrollBar = "dynamic";
      new GuiTextListCtrl(ServerList)
      {
         profile = ArenaListProfile; position = "2 2"; extent = "640 20"; visible = true; enabled = true;
         columns = "0 300 400 490";
      };
   };
   new GuiButtonCtrl(RefreshButton)
   {
      profile = ArenaButtonProfile; position = "70 470"; extent = "200 50"; visible = true; enabled = true;
      text = "Refresh"; command = "refreshServers();";
   };
   new GuiButtonCtrl(JoinSelectedButton)
   {
      profile = ArenaButtonProfile; position = "300 470"; extent = "200 50"; visible = true; enabled = true;
      text = "Join"; command = "joinSelected();";
   };
   new GuiButtonCtrl(BackButton)
   {
      profile = ArenaButtonProfile; position = "530 470"; extent = "200 50"; visible = true; enabled = true;
      text = "Back"; command = "showMainMenu();";
   };
   new GuiTextCtrl(JoinStatus)
   {
      profile = ArenaNoteProfile; position = "70 540"; extent = "660 28"; vertSizing = "top"; visible = true; enabled = true;
      text = "";
   };
};

function showJoin()
{
   saveName();
   Canvas.setContent(JoinGui);
   refreshServers();
}

function refreshServers()
{
   ServerList.clear();
   $Arena::ServerRows = 0;
   JoinStatus.setText("Looking for games...");
   queryLanServers($Arena::Port, 0, "", "", 0, 100, 0, 2, 0, 100, 0);
}

function onServerQueryStatus(%status, %message, %value)
{
   if (%status $= "done")
   {
      fillServerList();
      if (getServerCount() == 0)
         JoinStatus.setText("No games found. Host one, or refresh.");
      else
         JoinStatus.setText("Pick a game and press Join.");
   }
}

function fillServerList()
{
   ServerList.clear();
   for (%i = 0; %i < getServerCount(); %i++)
   {
      if (!setServerInfo(%i))
         continue;
      %players = $ServerInfo::PlayerCount @ " / " @ $ServerInfo::MaxPlayers;
      ServerList.addRow(%i, $ServerInfo::Name TAB %players TAB $ServerInfo::Ping TAB $ServerInfo::MissionName);
   }
   $Arena::ServerRows = ServerList.rowCount();
   if ($Arena::ServerRows > 0)
      ServerList.setSelectedRow(0);
}

function joinSelected()
{
   %row = ServerList.getSelectedId();
   if (%row $= "" || %row < 0 || !setServerInfo(%row))
   {
      JoinStatus.setText("Pick a game first.");
      return;
   }
   JoinStatus.setText("Joining " @ $ServerInfo::Name @ "...");
   connectTo($ServerInfo::Address);
}

// ---- the view

new GameTSCtrl(PlayGui)
{
   position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiTextCtrl(Crosshair)
   {
      profile = ArenaCrossProfile; position = "380 282"; extent = "40 36"; horizSizing = "center"; vertSizing = "center";
      visible = true; enabled = true; text = "+";
   };
   // The chat lines: ChatBox gets one row of two texts for each (a player's line, an arena line), made below.
   new GuiControl(ChatBox) { profile = ArenaBandProfile; position = "12 12"; extent = "460 132"; };
   new GuiTextEditCtrl(ChatEdit)
   {
      profile = ArenaEditProfile; position = "12 150"; extent = "460 30"; visible = false; enabled = true;
      maxLength = 120; text = "";
      altCommand = "sendChat();"; escapeCommand = "closeChat();";
   };
   new GuiTextCtrl(HealthHud)
   {
      profile = ArenaHealthProfile; position = "24 540"; extent = "220 46"; vertSizing = "top"; visible = true; enabled = true;
      text = "";
   };
   new GuiTextCtrl(AmmoHud)
   {
      profile = ArenaBigProfile; position = "556 540"; extent = "220 46"; horizSizing = "left"; vertSizing = "top";
      visible = true; enabled = true; text = "";
   };
   new GuiTextCtrl(PlayNote)
   {
      profile = ArenaNoteProfile; position = "100 200"; extent = "600 30"; horizSizing = "center"; visible = true; enabled = true;
      text = "";
   };
   new GuiControl(HelpBand)
   {
      profile = ArenaBandProfile; position = "500 12"; extent = "288 26"; horizSizing = "left";
      new GuiTextCtrl(PlayHelp)
      {
         profile = ArenaDimProfile; position = "10 4"; extent = "270 18"; visible = true; enabled = true;
         text = "Click shoot   T chat   Esc leave";
      };
   };
};

$Arena::ChatRows = 6;
for (%i = 0; %i < $Arena::ChatRows; %i++)
{
   %y = 6 + %i * 20;
   ChatBox.add(new GuiTextCtrl("ChatRow" @ %i)
   {
      profile = ArenaChatProfile; position = "10" SPC %y; extent = "440 20"; visible = true; enabled = true; text = "";
   });
   ChatBox.add(new GuiTextCtrl("InfoRow" @ %i)
   {
      profile = ArenaInfoProfile; position = "10" SPC %y; extent = "440 20"; visible = true; enabled = true; text = "";
   });
}

// The lines the chat box shows, the newest last: each line's tag is 1 for an arena line, 0 for a player's.
new MessageVector(ChatLines);

function chatLine(%info, %text)
{
   ChatLines.pushBackLine(%text, %info ? 1 : 0);
   $Arena::ChatCount++;
   $Arena::LastChat = %text;
   while (ChatLines.getNumLines() > $Arena::ChatRows)
      ChatLines.popFrontLine();
   showChat();
}

function showChat()
{
   %count = ChatLines.getNumLines();
   for (%i = 0; %i < $Arena::ChatRows; %i++)
   {
      %text = %i < %count ? ChatLines.getLineText(%i) : "";
      %info = %i < %count ? ChatLines.getLineTag(%i) : 0;
      %chat = "ChatRow" @ %i;
      %line = "InfoRow" @ %i;
      %chat.setText(%info ? "" : %text);
      %line.setText(%info ? %text : "");
   }
}

// A line in the middle of the view for a few seconds.
function showNote(%text)
{
   cancel($Arena::NoteTimer);
   PlayNote.setText(%text);
   $Arena::NoteTimer = schedule(2500, 0, "clearNote");
}

function clearNote()
{
   PlayNote.setText("");
}

// ---- chat

function openChat()
{
   if ($Arena::Chatting)
      return;
   $Arena::Chatting = 1;
   PlayMap.pop();
   ChatEdit.setValue("");
   ChatEdit.setVisible(true);
   ChatEdit.makeFirstResponder(true);
   // The key that opened the box types its letter into it a moment later: take it out again.
   schedule(1, 0, "clearChatEdit");
}

function clearChatEdit()
{
   ChatEdit.setValue("");
}

function closeChat()
{
   if (!$Arena::Chatting)
      return;
   $Arena::Chatting = 0;
   ChatEdit.setValue("");
   ChatEdit.setVisible(false);
   ChatEdit.makeFirstResponder(false);
   PlayMap.push();
}

function sendChat()
{
   %text = trim(ChatEdit.getValue());
   closeChat();
   if (%text !$= "")
      say(%text);
}

function say(%text)
{
   if (isObject(ServerConnection))
      commandToServer('Say', %text);
}

// ---- the keys

function resetCanvas() {}

new ActionMap(PlayMap);
function moveForward(%v) { $mvForwardAction = %v; }
function moveBack(%v) { $mvBackwardAction = %v; }
function moveLeft(%v) { $mvLeftAction = %v; }
function moveRight(%v) { $mvRightAction = %v; }
function jump(%v) { $mvTriggerCount2++; }
// The trigger counts once going down and once coming up.
function fire(%v) { $mvTriggerCount0++; }
// The view turns at once with the mouse ($Arena::LookYaw and $Arena::LookPitch); the moves carry the same turns to the server.
$Arena::MaxLook = 1.4;
function turn(%v) { $mvYaw += %v * 0.003; $Arena::LookYaw += %v * 0.003; }
function look(%v)
{
   %pitch = $Arena::LookPitch + %v * 0.003;
   if (%pitch > $Arena::MaxLook)
      %pitch = $Arena::MaxLook;
   if (%pitch < -$Arena::MaxLook)
      %pitch = -$Arena::MaxLook;
   $mvPitch += %pitch - $Arena::LookPitch;
   $Arena::LookPitch = %pitch;
}
function chatKey(%v) { if (%v) openChat(); }
function leaveKey(%v) { if (%v) leaveGame(); }
PlayMap.bind(keyboard, w, moveForward);
PlayMap.bind(keyboard, s, moveBack);
PlayMap.bind(keyboard, a, moveLeft);
PlayMap.bind(keyboard, d, moveRight);
PlayMap.bind(keyboard, space, jump);
PlayMap.bind(keyboard, t, chatKey);
PlayMap.bind(keyboard, escape, leaveKey);
PlayMap.bind(mouse, button0, fire);
PlayMap.bind(mouse, xaxis, turn);
PlayMap.bind(mouse, yaxis, look);

// ---- in and out

function showMainMenu()
{
   $Arena::InGame = 0;
   $Arena::TextureFilter = "";
   PlayMap.pop();
   NameEdit.setValue($Pref::Player::Name);
   Canvas.setContent(MainMenuGui);
   Canvas.cursorOn();
}

function connectTo(%address)
{
   if (isObject(ServerConnection))
      ServerConnection.delete();
   %conn = new GameConnection(ServerConnection);
   %conn.setConnectArgs($Pref::Player::Name);
   %conn.connect(%address);
}

function leaveGame()
{
   if ($Arena::Chatting)
      closeChat();
   if (isObject(ServerConnection))
      ServerConnection.delete();
   $mvForwardAction = $mvBackwardAction = $mvLeftAction = $mvRightAction = 0;
   $Arena::OwnGhost = -1;
   ChatLines.clear();
   showChat();
   showMainMenu();
   $Arena::LeaveAsked = 1;
}

function onDataBlockObjectReceived(%index, %total)
{
   MainStatus.setText("Loading... " @ %index + 1 @ " of " @ %total);
}

function onGhostAlwaysStarted(%count)
{
   MainStatus.setText("Loading the arena...");
}

function GameConnection::onConnectionAccepted(%this)
{
   MainStatus.setText("Joined. Waiting for the arena...");
   JoinStatus.setText("Joined. Waiting for the arena...");
}

function GameConnection::onConnectRequestRejected(%this, %reason)
{
   JoinStatus.setText("The game turned us away: " @ %reason);
   MainStatus.setText("The game turned us away: " @ %reason);
}

function GameConnection::onConnectRequestTimedOut(%this)
{
   JoinStatus.setText("The game did not answer.");
   MainStatus.setText("The game did not answer.");
}

function GameConnection::onConnectionDropped(%this, %reason)
{
   leaveGame();
   MainStatus.setText("Left the game: " @ %reason);
}

function GameConnection::onConnectionTimedOut(%this)
{
   leaveGame();
   MainStatus.setText("Lost the connection to the game.");
}

// ---- what the server says

// %textureFilter is the map's choice of how its textures are drawn ("nearest": sharp texels; empty: smooth).
function clientCmdArenaStarted(%map, %server, %textureFilter)
{
   $Arena::TextureFilter = %textureFilter;
   MainStatus.setText("");
   JoinStatus.setText("");
   Canvas.setContent(PlayGui);
   Canvas.cursorOff();
   PlayMap.push();
   $Arena::InGame = 1;
   showNote(%server);
}

function clientCmdOwnFigure(%ghost, %yaw)
{
   $Arena::OwnGhost = %ghost;
   $Arena::LookYaw = %yaw;
   $Arena::LookPitch = 0;
}

function clientCmdArenaMessage(%kind, %a, %b)
{
   if (%kind $= "chat")
      chatLine(0, %a @ ": " @ %b);
   else
      chatLine(1, %a);
}

function clientCmdSetAmmo(%left, %full)
{
   $Arena::Ammo = %left;
   AmmoHud.setText(%left @ " / " @ %full);
   if (%left == 0)
      showNote("Out of rounds");
}

function clientCmdSetHealth(%left, %full)
{
   $Arena::Health = %left;
   HealthHud.setText("+" SPC %left);
}

function clientCmdHit(%name)
{
   $Arena::Hits++;
}

function clientCmdKnockedOut(%ms)
{
   showNote("Knocked out. Back in a moment...");
}

// ---- the screen's hurt and hit marks (the client draws them from these: a red edge and a mark toward %from when hurt, an X
// over the crosshair on a hit)

function clientCmdHurt(%amount, %from)
{
   $Arena::HurtCount++;
   $Arena::HurtFrom = %from;
}

$Pref::Player::Name = "Player";
// Ask the server for a packet every tick (it sends at the slower of its rate and this one).
$pref::Net::PacketRateToClient = 32;
$Arena::OwnGhost = -1;
showMainMenu();
