// The arena's client: the main menu, the list of games on the local network, the view with its HUD (rounds, health, the
// round's clock, chat), the pause menu, the round's scores, the keys, and the way in and out.
//
// The host watches the globals the scripts set: $Arena::HostAsked (start a server here and join it), $Arena::LeaveAsked (the
// view was left: stop hosting), $Arena::JoinAsked (a browser's client: join the page's game again) and $Arena::QuitAsked. $Arena::Port is the UDP port number the games listen on. $Arena::HostScope
// ("lan" or "public") says who may join the game the host starts.

exec("common/client.cs");

// ---- profiles

new GuiControlProfile(ArenaClearProfile) { opaque = false; };
new GuiControlProfile(ArenaBackProfile) { opaque = true; fillColor = "20 24 32 255"; };
new GuiControlProfile(ArenaPanelProfile) { opaque = true; border = true; fillColor = "30 36 48 235"; borderColor = "70 82 104 255"; };
new GuiControlProfile(ArenaBandProfile) { opaque = true; fillColor = "10 12 18 150"; };
new GuiControlProfile(ArenaTitleProfile) { fontType = "Arial"; fontSize = 48; fontColors[0] = "246 240 228 255"; };
new GuiControlProfile(ArenaTextProfile) { fontType = "Arial"; fontSize = 18; fontColors[0] = "226 230 238 255"; };
new GuiControlProfile(ArenaDimProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "150 160 178 255"; };
new GuiControlProfile(ArenaNoteProfile) { fontType = "Arial"; fontSize = 24; fontColors[0] = "255 208 120 255"; justify = "center"; };
new GuiControlProfile(ArenaHeadProfile) { fontType = "Arial"; fontSize = 32; fontColors[0] = "246 240 228 255"; justify = "center"; };
new GuiControlProfile(ArenaReloadProfile) { fontType = "Arial"; fontSize = 22; fontColors[0] = "255 208 120 255"; justify = "right"; };
new GuiControlProfile(ArenaHintProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "150 160 178 255"; justify = "center"; };
new GuiControlProfile(ArenaShadeProfile) { opaque = true; fillColor = "8 10 16 140"; };
new GuiControlProfile(ArenaBigProfile) { fontType = "Arial"; fontSize = 40; fontColors[0] = "255 236 200 255"; justify = "right"; };
new GuiControlProfile(ArenaHealthProfile) { fontType = "Arial"; fontSize = 40; fontColors[0] = "170 232 170 255"; };
new GuiControlProfile(ArenaCrossProfile) { fontType = "Arial"; fontSize = 28; fontColors[0] = "255 255 255 220"; justify = "center"; };
new GuiControlProfile(ArenaChatProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "236 240 248 255"; };
new GuiControlProfile(ArenaInfoProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "255 208 120 255"; };
new GuiControlProfile(ArenaClockProfile) { fontType = "Arial"; fontSize = 28; fontColors[0] = "236 240 248 255"; justify = "center"; };
new GuiControlProfile(ArenaRowProfile) { fontType = "Arial"; fontSize = 22; fontColors[0] = "226 230 238 255"; };
new GuiControlProfile(ArenaScoreProfile) { fontType = "Arial"; fontSize = 22; fontColors[0] = "255 236 200 255"; justify = "right"; };
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
   new GuiControl(MainPanel)
   {
      profile = ArenaClearProfile; position = "50 50"; extent = "720 500"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(MainTitle)
      {
         profile = ArenaTitleProfile; position = "20 20"; extent = "660 56"; visible = true; enabled = true;
         text = "Arena";
      };
      new GuiTextCtrl(MainLine)
      {
         profile = ArenaDimProfile; position = "22 80"; extent = "660 20"; visible = true; enabled = true;
         text = "A small shooter for one player or a few on the same network.";
      };
      new GuiTextCtrl(NameLabel)
      {
         profile = ArenaTextProfile; position = "22 136"; extent = "110 24"; visible = true; enabled = true;
         text = "Your name";
      };
      new GuiTextEditCtrl(NameEdit)
      {
         profile = ArenaEditProfile; position = "140 132"; extent = "240 30"; visible = true; enabled = true;
         maxLength = 24; text = "";
         altCommand = "saveName();";
      };
      new GuiButtonCtrl(HostButton)
      {
         profile = ArenaButtonProfile; position = "20 200"; extent = "280 56"; visible = true; enabled = true;
         text = "Host game"; command = "showHost();";
      };
      new GuiButtonCtrl(JoinButton)
      {
         profile = ArenaButtonProfile; position = "20 272"; extent = "280 56"; visible = true; enabled = true;
         text = "Join game"; command = "showJoin();";
      };
      new GuiButtonCtrl(QuitButton)
      {
         profile = ArenaButtonProfile; position = "20 344"; extent = "280 56"; visible = true; enabled = true;
         text = "Quit"; command = "quitGame();";
      };
      new GuiTextCtrl(HostNote)
      {
         profile = ArenaDimProfile; position = "322 218"; extent = "380 20"; visible = true; enabled = true;
         text = "Start a game here. Others on your network can join it.";
      };
      new GuiTextCtrl(JoinNote)
      {
         profile = ArenaDimProfile; position = "322 290"; extent = "380 20"; visible = true; enabled = true;
         text = "Find a game someone is hosting on your network.";
      };
      new GuiTextCtrl(MainStatus)
      {
         profile = ArenaNoteProfile; position = "20 470"; extent = "660 28"; vertSizing = "top"; visible = true; enabled = true;
         text = "";
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

// ---- the choice of who may join a hosted game

new GuiControl(HostGui)
{
   profile = ArenaBackProfile; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(HostPanel)
   {
      profile = ArenaClearProfile; position = "50 40"; extent = "740 520"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(HostTitle)
      {
         profile = ArenaTitleProfile; position = "20 10"; extent = "660 56"; visible = true; enabled = true;
         text = "Host a game";
      };
      new GuiTextCtrl(HostLine)
      {
         profile = ArenaDimProfile; position = "22 72"; extent = "660 20"; visible = true; enabled = true;
         text = "Who can join the game you start?";
      };
      new GuiButtonCtrl(HostLanButton)
      {
         profile = ArenaButtonProfile; position = "20 130"; extent = "280 56"; visible = true; enabled = true;
         text = "LAN only"; command = "hostGame(\"lan\");";
      };
      new GuiTextCtrl(HostLanNote)
      {
         profile = ArenaDimProfile; position = "322 132"; extent = "400 20"; visible = true; enabled = true;
         text = "Players on your own network can join.";
      };
      new GuiTextCtrl(HostLanNote2)
      {
         profile = ArenaDimProfile; position = "322 154"; extent = "400 20"; visible = true; enabled = true;
         text = "Nobody outside it can reach the game.";
      };
      new GuiButtonCtrl(HostPublicButton)
      {
         profile = ArenaButtonProfile; position = "20 210"; extent = "280 56"; visible = true; enabled = true;
         text = "Public"; command = "hostGame(\"public\");";
      };
      new GuiTextCtrl(HostPublicNote)
      {
         profile = ArenaDimProfile; position = "322 212"; extent = "400 20"; visible = true; enabled = true;
         text = "Anyone who can reach this computer can join.";
      };
      new GuiTextCtrl(HostPublicNote2)
      {
         profile = ArenaDimProfile; position = "322 234"; extent = "400 20"; visible = true; enabled = true;
         text = "Needs UDP 28000 forwarded to this computer.";
      };
      new GuiButtonCtrl(HostBackButton)
      {
         profile = ArenaButtonProfile; position = "20 290"; extent = "280 56"; visible = true; enabled = true;
         text = "Back"; command = "showMainMenu();";
      };
      new GuiTextCtrl(HostLast)
      {
         profile = ArenaDimProfile; position = "22 370"; extent = "660 20"; visible = true; enabled = true;
         text = "";
      };
      new GuiTextCtrl(HostStatus)
      {
         profile = ArenaNoteProfile; position = "20 480"; extent = "660 28"; vertSizing = "top"; visible = true; enabled = true;
         text = "";
      };
   };
};

function showHost()
{
   saveName();
   HostStatus.setText("");
   HostLast.setText($Pref::Arena::HostScope $= "public" ? "Last time you hosted: Public." : "Last time you hosted: LAN only.");
   Canvas.setContent(HostGui);
}

// %scope is "lan" or "public"; empty takes the last choice.
function hostGame(%scope)
{
   if (%scope $= "")
      %scope = $Pref::Arena::HostScope;
   if (%scope !$= "public")
      %scope = "lan";
   $Pref::Arena::HostScope = %scope;
   $Arena::HostScope = %scope;
   saveName();
   MainStatus.setText("Starting a game...");
   HostStatus.setText("Starting a game...");
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
   // The content is one panel the script sizes to the window (layoutJoin: as wide as the window up to a comfortable width,
   // centered); what is in it follows the panel's size by its own sizing.
   new GuiControl(JoinPanel)
   {
      profile = ArenaClearProfile; position = "0 0"; extent = "1000 540";
      new GuiTextCtrl(JoinTitle)
      {
         profile = ArenaTitleProfile; position = "0 0"; extent = "700 56"; visible = true; enabled = true;
         text = "Join a game";
      };
      new GuiTextCtrl(JoinHeaderGame)
      {
         profile = ArenaDimProfile; position = "4 74"; extent = "200 20"; visible = true; enabled = true; text = "Game";
      };
      new GuiTextCtrl(JoinHeaderPlayers)
      {
         profile = ArenaDimProfile; position = "464 74"; extent = "90 20"; visible = true; enabled = true; text = "Players";
      };
      new GuiTextCtrl(JoinHeaderPing)
      {
         profile = ArenaDimProfile; position = "584 74"; extent = "80 20"; visible = true; enabled = true; text = "Ping";
      };
      new GuiTextCtrl(JoinHeaderMap)
      {
         profile = ArenaDimProfile; position = "704 74"; extent = "200 20"; visible = true; enabled = true; text = "Map";
      };
      new GuiScrollCtrl(ServerScroll)
      {
         profile = ArenaScrollProfile; position = "0 98"; extent = "1000 252"; horizSizing = "width"; vertSizing = "height";
         visible = true; enabled = true; hScrollBar = "alwaysOff"; vScrollBar = "dynamic";
         new GuiTextListCtrl(ServerList)
         {
            profile = ArenaListProfile; position = "2 2"; extent = "980 20"; visible = true; enabled = true;
            columns = "0 460 580 700";
         };
      };
      new GuiTextCtrl(AddressLabel)
      {
         profile = ArenaTextProfile; position = "4 372"; extent = "76 24"; vertSizing = "top"; visible = true; enabled = true;
         text = "Address";
      };
      new GuiTextEditCtrl(AddressEdit)
      {
         profile = ArenaEditProfile; position = "82 368"; extent = "738 32"; horizSizing = "width"; vertSizing = "top";
         visible = true; enabled = true; maxLength = 260; text = ""; placeholderText = "127.0.0.1:28000";
         altCommand = "connectAddress();";
      };
      new GuiButtonCtrl(ConnectButton)
      {
         profile = ArenaButtonProfile; position = "830 364"; extent = "170 40"; horizSizing = "left"; vertSizing = "top";
         visible = true; enabled = true; text = "Connect"; command = "connectAddress();";
      };
      new GuiButtonCtrl(RefreshButton)
      {
         profile = ArenaButtonProfile; position = "0 440"; extent = "200 50"; vertSizing = "top"; visible = true; enabled = true;
         text = "Refresh"; command = "refreshServers();";
      };
      new GuiButtonCtrl(JoinSelectedButton)
      {
         profile = ArenaButtonProfile; position = "400 440"; extent = "200 50"; horizSizing = "center"; vertSizing = "top";
         visible = true; enabled = true; text = "Join"; command = "joinSelected();";
      };
      new GuiButtonCtrl(BackButton)
      {
         profile = ArenaButtonProfile; position = "800 440"; extent = "200 50"; horizSizing = "left"; vertSizing = "top";
         visible = true; enabled = true; text = "Back"; command = "showMainMenu();";
      };
      new GuiTextCtrl(JoinStatus)
      {
         profile = ArenaNoteProfile; position = "0 506"; extent = "1000 28"; horizSizing = "width"; vertSizing = "top";
         visible = true; enabled = true; text = "";
      };
   };
};

// The join panel at the window's size: as wide as the window less a margin, up to 1000 (then centered), as tall up to 760.
function layoutJoin()
{
   %size = Canvas.getExtent();
   %cw = getWord(%size, 0);
   %ch = getWord(%size, 1);
   %w = %cw - 64;
   if (%w > 1000) %w = 1000;
   if (%w < 640) %w = 640;
   %h = %ch - 40;
   if (%h > 760) %h = 760;
   if (%h < 500) %h = 500;
   JoinPanel.resize(mFloor((%cw - %w) / 2), mFloor((%ch - %h) / 2), %w, %h);
   // The list's columns and their headers keep their share of the width.
   %players = mFloor(%w * 46 / 100);
   %ping = mFloor(%w * 58 / 100);
   %map = mFloor(%w * 70 / 100);
   ServerList.columns = "0" SPC %players SPC %ping SPC %map;
   JoinHeaderPlayers.resize(4 + %players, 74, 90, 20);
   JoinHeaderPing.resize(4 + %ping, 74, 80, 20);
   JoinHeaderMap.resize(4 + %map, 74, %w - %map - 8, 20);
}

// The scripts' own re-layout after the window changed size.
function resetCanvas()
{
   layoutJoin();
}

function showJoin()
{
   saveName();
   AddressEdit.setValue($Pref::Client::LastAddress);
   Canvas.setContent(JoinGui);
   layoutJoin();
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
   cancelJoinLookup();
   JoinStatus.setText("Joining " @ $ServerInfo::Name @ "...");
   connectTo($ServerInfo::Address);
}

// Connects to the address or name typed in the box (with :port, or without: port 28000); a name is looked up first.
function connectAddress()
{
   saveName();
   %typed = trim(AddressEdit.getValue());
   %address = checkJoinAddress(%typed);
   if (getSubStr(%address, 0, 1) $= "!")
   {
      JoinStatus.setText(getSubStr(%address, 1, 200));
      return;
   }
   $Pref::Client::LastAddress = %typed;
   if (getSubStr(%address, 0, 1) $= "?")
   {
      JoinStatus.setText("Looking up " @ getSubStr(%address, 1, 260) @ "...");
      schedule(50, 0, pollJoinLookup);
      return;
   }
   JoinStatus.setText("Connecting to " @ %address @ "...");
   armPublicJoin(%address);
   connectTo(%address);
}

// Connects once the typed name's lookup ends; leaving the screen drops it.
function pollJoinLookup()
{
   if (Canvas.getContent() != JoinGui.getId())
   {
      cancelJoinLookup();
      return;
   }
   %address = joinLookupResult();
   if (%address $= "?")
      schedule(50, 0, pollJoinLookup);
   else if (getSubStr(%address, 0, 1) $= "!")
      JoinStatus.setText(getSubStr(%address, 1, 200));
   else if (%address !$= "")
   {
      JoinStatus.setText("Connecting to " @ %address @ "...");
      connectTo(%address);
   }
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
   new GuiTextCtrl(ReloadHud)
   {
      profile = ArenaReloadProfile; position = "556 512"; extent = "220 28"; horizSizing = "left"; vertSizing = "top";
      visible = true; enabled = true; text = "";
   };
   new GuiTextCtrl(RoundHud)
   {
      profile = ArenaClockProfile; position = "300 548"; extent = "200 36"; horizSizing = "center"; vertSizing = "top";
      visible = true; enabled = true; text = "";
   };
   new GuiTextCtrl(PlayNote)
   {
      profile = ArenaNoteProfile; position = "100 200"; extent = "600 30"; horizSizing = "center"; visible = true; enabled = true;
      text = "";
   };
   new GuiControl(HelpBand)
   {
      profile = ArenaBandProfile; position = "328 12"; extent = "460 46"; horizSizing = "left";
      new GuiTextCtrl(PlayHelp)
      {
         profile = ArenaDimProfile; position = "10 4"; extent = "442 18"; visible = true; enabled = true;
         text = "Click shoot   R reload   Tab camera   T chat   Esc menu";
      };
      new GuiTextCtrl(PoseHelp)
      {
         profile = ArenaDimProfile; position = "10 24"; extent = "442 18"; visible = true; enabled = true;
         text = "W A S D walk   Shift sprint   Ctrl crouch   C toggle crouch   Z prone   Space jump";
      };
   };
};

// ---- the pause menu (common/pause.cs): over the view (the game goes on: others still play), the cursor on and the keys off
// the figure

exec("common/pause.cs");

function pauseAllowed() { return !$Arena::Chatting && $Arena::InGame; }
function onPause()
{
   PlayMap.pop();
   releaseKeys();
}
function onResume() { PlayMap.push(); }

// ---- the round's end: its scores over the view until the next map is in (the game goes on underneath, without shots)

new GuiControl(RoundEndGui)
{
   profile = ArenaShadeProfile; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(RoundPanel)
   {
      profile = ArenaPanelProfile; position = "220 110"; extent = "360 380"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(RoundTitle)
      {
         profile = ArenaHeadProfile; position = "0 22"; extent = "360 40"; visible = true; enabled = true;
         text = "Round over";
      };
      new GuiTextCtrl(RoundNext)
      {
         profile = ArenaHintProfile; position = "0 336"; extent = "360 22"; visible = true; enabled = true;
         text = "";
      };
   };
};

// The board's rows: a name and a score each, the highest first.
$Arena::BoardRows = 8;
function makeBoardRows()
{
   for (%i = 0; %i < $Arena::BoardRows; %i++)
   {
      %y = 80 + %i * 30;
      RoundPanel.add(new GuiTextCtrl("BoardName" @ %i)
      {
         profile = ArenaRowProfile; position = "40" SPC %y; extent = "220 28"; visible = true; enabled = true; text = "";
      });
      RoundPanel.add(new GuiTextCtrl("BoardScore" @ %i)
      {
         profile = ArenaScoreProfile; position = "260" SPC %y; extent = "60 28"; visible = true; enabled = true; text = "";
      });
   }
}
makeBoardRows();

// Shows the round's scores (%board: a record of "name" TAB score for each player, the highest first) and the map that comes
// next in %seconds.
function showRoundEnd(%next, %seconds, %board)
{
   %count = getRecordCount(%board);
   for (%i = 0; %i < $Arena::BoardRows; %i++)
   {
      %row = %i < %count ? getRecord(%board, %i) : "";
      %name = "BoardName" @ %i;
      %score = "BoardScore" @ %i;
      %name.setText(%row $= "" ? "" : %i + 1 @ ".  " @ getField(%row, 0));
      %score.setText(getField(%row, 1));
   }
   RoundNext.setText("Next map: " @ %next @ " in " @ %seconds @ " s");
   if (!$Arena::RoundShown)
      Canvas.pushDialog(RoundEndGui);
   $Arena::RoundShown = 1;
   sizeRoundPanel(%count);
}

// The panel as tall as its rows of scores (one at least), the next map's line under them, in the middle of the view.
function sizeRoundPanel(%rows)
{
   %rows = mClamp(%rows, 1, $Arena::BoardRows);
   %next = 80 + %rows * 30 + 14;
   %height = %next + 22 + 22;
   %view = RoundEndGui.getExtent();
   %x = mFloor((getWord(%view, 0) - 360) / 2);
   %y = mFloor((getWord(%view, 1) - %height) / 2);
   RoundPanel.resize(%x, %y, 360, %height);
   RoundNext.resize(0, %next, 360, 22);
}

function hideRoundEnd()
{
   if (!$Arena::RoundShown)
      return;
   $Arena::RoundShown = 0;
   Canvas.popDialog(RoundEndGui);
}

// The round's clock under the view: minutes and seconds left, counted down here between the server's word.
function setRoundClock(%seconds)
{
   cancel($Arena::ClockTimer);
   $Arena::RoundLeft = %seconds;
   showRoundClock();
}

function showRoundClock()
{
   %left = $Arena::RoundLeft;
   if (%left <= 0)
   {
      RoundHud.setText("");
      return;
   }
   %m = mFloor(%left / 60);
   %s = %left % 60;
   RoundHud.setText(%m @ ":" @ (%s < 10 ? "0" : "") @ %s);
   $Arena::RoundLeft = %left - 1;
   $Arena::ClockTimer = schedule(1000, 0, "showRoundClock");
}

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

new ActionMap(PlayMap);
// The keys held, and the moves made of them: a walk (a share of the full speed), the full speed while Shift is held (a
// sprint), and no faster than $Arena::AimWalk while aiming down the sights.
$Arena::AimWalk = 0.6;
$Arena::WalkPace = 0.35;
function applyMoves()
{
   %speed = $Arena::Sprint ? 1 : $Arena::WalkPace;
   if ($Arena::Aiming && %speed > $Arena::AimWalk)
      %speed = $Arena::AimWalk;
   $mvForwardAction = $Arena::KeyForward * %speed;
   $mvBackwardAction = $Arena::KeyBack * %speed;
   $mvLeftAction = $Arena::KeyLeft * %speed;
   $mvRightAction = $Arena::KeyRight * %speed;
}
function releaseKeys()
{
   $Arena::KeyForward = $Arena::KeyBack = $Arena::KeyLeft = $Arena::KeyRight = 0;
   $Arena::Sprint = 0;
   if ($Arena::Aiming)
      aim(0);
   applyMoves();
   releasePoseKeys();
}
function moveForward(%v) { $Arena::KeyForward = %v; applyMoves(); }
function moveBack(%v) { $Arena::KeyBack = %v; applyMoves(); }
function moveLeft(%v) { $Arena::KeyLeft = %v; applyMoves(); }
function moveRight(%v) { $Arena::KeyRight = %v; applyMoves(); }
function sprint(%v) { $Arena::Sprint = %v != 0; applyMoves(); }
function jump(%v) { if (!poseJumpKey(%v)) $mvTriggerCount2++; }
// The trigger counts once going down and once coming up.
function fire(%v) { $mvTriggerCount0++; }
// Aiming down the sights while the right button is held: the move's trigger 4 tells the server (the figure raises its pistol
// for the others to see); the client moves the view's pistol to the sights and narrows the view ($Arena::Aiming), and the
// crosshair gives way to the sights.
function aim(%v)
{
   if ((%v != 0) == ($Arena::Aiming != 0))
      return;
   $mvTriggerCount4++;
   $Arena::Aiming = %v != 0;
   Crosshair.setVisible(!$Arena::Aiming);
   applyMoves();
}
// The view turns at once with the mouse ($Arena::LookYaw and $Arena::LookPitch); the moves carry the same turns to the server.
// A narrower view turns slower by as much ($Arena::TurnScale, which the client sets from the field of view each frame), so a
// move of the mouse sweeps the same share of the screen.
$Arena::MaxLook = 1.4;
$Arena::TurnScale = 1;
function turn(%v) { %d = %v * 0.003 * $Arena::TurnScale; $mvYaw += %d; $Arena::LookYaw += %d; }
function look(%v)
{
   %pitch = $Arena::LookPitch + %v * 0.003 * $Arena::TurnScale;
   if (%pitch > $Arena::MaxLook)
      %pitch = $Arena::MaxLook;
   if (%pitch < -$Arena::MaxLook)
      %pitch = -$Arena::MaxLook;
   $mvPitch += %pitch - $Arena::LookPitch;
   $Arena::LookPitch = %pitch;
}
// Tab: the camera behind the figure or at its eyes.
function switchCamera(%v) { if (%v) $Arena::ThirdPerson = !$Arena::ThirdPerson; }
function chatKey(%v) { if (%v) openChat(); }
function reloadKey(%v) { if (%v && isObject(ServerConnection)) commandToServer('Reload'); }
PlayMap.bind(keyboard, w, moveForward);
PlayMap.bind(keyboard, s, moveBack);
PlayMap.bind(keyboard, a, moveLeft);
PlayMap.bind(keyboard, d, moveRight);
PlayMap.bind(keyboard, space, jump);
PlayMap.bind(keyboard, lshift, sprint);
bindPoseKeys(PlayMap);
PlayMap.bind(keyboard, tab, switchCamera);
PlayMap.bind(keyboard, t, chatKey);
PlayMap.bind(keyboard, r, reloadKey);
PlayMap.bind(keyboard, escape, pauseKey);
PlayMap.bind(mouse, button0, fire);
PlayMap.bind(mouse, button1, aim);
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
   if ($Arena::JoinOnly)
      showJoinOnly();
}

// A browser's client ($Arena::JoinOnly) joins the one game its page came from: the main menu offers that and nothing else.
// $Arena::JoinAsked asks the host to open a new connection to that game and join it. Played alone ($Arena::Solo: the page's
// own game, nobody else in it), the menu offers to play again.
function showJoinOnly()
{
   HostButton.setVisible(0);
   HostNote.setVisible(0);
   QuitButton.setVisible(0);
   JoinButton.command = "joinPage();";
   if ($Arena::Solo)
   {
      MainLine.setText("A small shooter, played alone on this page.");
      JoinButton.setText("Play alone");
      JoinNote.setText("Back into the arena.");
      return;
   }
   JoinButton.setText("Join the game");
   JoinNote.setText("Join the game this page came from.");
}

function joinPage()
{
   saveName();
   MainStatus.setText("Joining...");
   $Arena::JoinAsked = 1;
}

// The lines the hosting client sets for the view's chat box: where the others join, and what a public game needs.
function showHostInfo()
{
   for (%i = 1; %i <= 3; %i++)
   {
      %line = $Arena::HostInfo[%i];
      if (%line !$= "")
         chatLine(1, %line);
      $Arena::HostInfo[%i] = "";
   }
}

function leaveGame()
{
   if ($Arena::Chatting)
      closeChat();
   closePause();
   hideRoundEnd();
   setRoundClock(0);
   disconnect();
   releaseKeys();
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

// %textureFilter is the map's choice of how its textures are drawn ("nearest": sharp texels; empty: smooth). It comes on
// each map: the first when the client joined, the next ones when the map changed (the client stayed in the game).
function clientCmdArenaStarted(%map, %server, %textureFilter)
{
   %changed = $Arena::InGame;
   $Arena::TextureFilter = %textureFilter;
   $Arena::Map = %map;
   $Arena::Maps++;
   MainStatus.setText("");
   JoinStatus.setText("");
   hideRoundEnd();
   if (%changed)
   {
      showNote(%map);
      return;
   }
   Canvas.setContent(PlayGui);
   Canvas.cursorOff();
   PlayMap.push();
   $Arena::InGame = 1;
   showHostInfo();
   showNote(%server);
}

// The map ended: its ghosts are going, the next map's come after its datablocks.
function clientCmdMissionEnd(%sequence)
{
   $Arena::OwnGhost = -1;
   $Arena::MissionEnds++;
   setRoundClock(0);
   RoundNext.setText("Loading the next map...");
}

// The seconds left in the round.
function clientCmdRoundTime(%seconds)
{
   setRoundClock(%seconds);
}

// The round is over: its scores, and the next map's name in %seconds.
function clientCmdRoundEnd(%next, %seconds, %board)
{
   $Arena::RoundEnds++;
   $Arena::Board = %board;
   setRoundClock(0);
   showRoundEnd(%next, %seconds, %board);
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
   if (%left == %full)
   {
      $Arena::Reloading = 0;
      ReloadHud.setText("");
   }
}

function clientCmdReloading(%seconds)
{
   $Arena::Reloading = 1;
   $Arena::Reloads++;
   ReloadHud.setText("Reloading...");
}

function clientCmdSetHealth(%left, %full)
{
   $Arena::Health = %left;
   HealthHud.setText("+" SPC %left);
}

// %part is the part of the figure the bolt hit (head, torso, rarm, ...).
function clientCmdHit(%name, %part)
{
   $Arena::Hits++;
   $Arena::HitPart = %part;
}

function clientCmdKnockedOut(%ms)
{
   showNote("Knocked out. Back in a moment...");
   // The next figure starts standing.
   releasePoseKeys();
}

// ---- the screen's hurt and hit marks (the client draws them from these: a red edge and a mark toward %from when hurt, an X
// over the crosshair on a hit)

function clientCmdHurt(%amount, %from)
{
   $Arena::HurtCount++;
   $Arena::HurtFrom = %from;
}

$Pref::Player::Name = "Player";
$Pref::Arena::HostScope = "lan";
$Pref::Client::LastAddress = "";
// Ask the server for a packet every tick (it sends at the slower of its rate and this one).
$pref::Net::PacketRateToClient = 32;
$Arena::OwnGhost = -1;
showMainMenu();
