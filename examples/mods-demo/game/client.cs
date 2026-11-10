// The demo's client: the main menu (host a match here, join one from the network's list of games or by address), the screen
// that shows the server's mods downloading, the match as the server sends it with a small board over the world, the keys and
// the Esc menu. The client has none of the game mode's rules: it is told its team, the score and the clock, and shows them.
// Its player is steered with the move globals ($mvForwardAction and the others), set by the keys or, headless, by a bot.
//
// The host watches the globals the scripts set: $Game::HostAsked (start a server here and join it; $Game::HostScope is
// "lan" or "public"), $Game::LeaveAsked (the match was left: stop hosting), $Game::JoinAsked (a browser's client: join the
// page's server) and $Game::QuitAsked. $Game::Port is the UDP port number the list of games asks on; $Game::JoinOnly hides
// what a browser cannot do (host, quit). $Join::State says where a join stands: connecting, accepted, downloading, cached,
// joining, playing or failed.

exec("prefs.cs");

// Ask the server for a packet every tick, as large as the wire's rate record allows: the ball moves fast enough for the
// default ten packets a second to show.
$pref::Net::PacketRateToClient = 32;
$pref::Net::PacketSize = 1023;

// The client looks on from outside its player, so its own player is drawn like the other.
$firstPerson = 0;

if ($Pref::Player::Name $= "")
   $Pref::Player::Name = "Player";

// ---- joining

// Joins the server at %address; the join screen names it %shown when given (a page names the server it came from, not the
// address its game socket stands for).
function connectTo(%address, %shown)
{
   disconnect();
   $Join::Address = %address;
   $Join::Shown = %shown $= "" ? %address : %shown;
   $Join::Mods = "";
   $Join::Files = 0;
   $Join::Bytes = 0;
   $Join::Received = 0;
   $Match::Team = "";
   $Match::Updates = "";
   setJoinState("connecting");
   %conn = new GameConnection(ServerConnection);
   if ($Pref::Player::Name !$= "")
      %conn.setConnectArgs($Pref::Player::Name);
   %conn.connect(%address);
   showJoinScreen();
}

function disconnect()
{
   if (isObject(ServerConnection))
      ServerConnection.delete();
}

function GameConnection::onConnectionAccepted(%this)
{
   setJoinState("accepted");
}

function GameConnection::onConnectRequestRejected(%this, %why)
{
   $Why = %why;
   joinFailed("The server turned you away: " @ %why);
}

function GameConnection::onConnectRequestTimedOut(%this)
{
   joinFailed("No server answered at " @ $Join::Shown @ ".");
}

function GameConnection::onConnectionDropped(%this, %reason)
{
   leftMatch("The server closed the connection: " @ %reason);
}

function GameConnection::onConnectionTimedOut(%this)
{
   leftMatch("Lost the connection to the server.");
}

// The mods the server needs, as "id version" each, and their files, from the offer it sent.
function readOffer()
{
   %list = getRequiredModList();
   $Join::Mods = "";
   $Join::Files = 0;
   for (%i = 0; %i < getWordCount(%list); %i++)
   {
      %id = getWord(%list, %i);
      %mod = %id SPC getRequiredModField(%id, "version");
      $Join::Mods = $Join::Mods $= "" ? %mod : $Join::Mods @ ", " @ %mod;
      $Join::Files += getRequiredModField(%id, "files");
   }
}

function onModDownloadStarted(%mods, %bytes)
{
   $Download::Started = %mods SPC %bytes;
   readOffer();
   $Join::Bytes = %bytes;
   $Join::Received = 0;
   setJoinState(%mods > 0 ? "downloading" : "cached");
}

function onModDownloadProgress(%id, %received, %total, %index, %count)
{
   $Join::Received = %received;
   $Join::Bytes = %total;
   updateJoinScreen();
}

function onModDownloadDone()
{
   $Download::Done++;
   $Join::Received = $Join::Bytes;
   if ($Join::State !$= "cached")
      setJoinState("joining");
}

function onModDownloadFailed(%why)
{
   $Download::Failed = %why;
   joinFailed("The download stopped: " @ %why);
}

function joinFailed(%why)
{
   $Join::Why = %why;
   setJoinState("failed");
}

// The connection ended: in the match, back to the menu with why; while joining, on the join screen.
function leftMatch(%why)
{
   if ($Join::State $= "playing")
   {
      leaveMatch();
      MainStatus.setText(%why);
      return;
   }
   joinFailed(%why);
}

// The join screen's Back, and the Esc menu's Leave: out of the server, back to the menu.
function leaveMatch()
{
   disconnect();
   releaseKeys();
   closePause();
   $Join::State = "";
   $Game::LeaveAsked = 1;
   showMainMenu();
}

// A size in bytes as a player reads it.
function sizeText(%bytes)
{
   if (%bytes < 1024)
      return %bytes @ " bytes";
   if (%bytes < 1048576)
      return mFloor(%bytes / 102.4) / 10 @ " KB";
   return mFloor(%bytes / 104857.6) / 10 @ " MB";
}

function setJoinState(%state)
{
   $Join::State = %state;
   updateJoinScreen();
}

// The join screen as the join stands now.
function updateJoinScreen()
{
   %state = $Join::State;
   JoinScreenTitle.setText("Joining " @ $Join::Shown);
   %offer = %state $= "downloading" || %state $= "cached" || %state $= "joining";
   JoinScreenMod.setText(%offer ? $Join::Mods : "");
   %files = $Join::Files == 1 ? "1 file" : $Join::Files @ " files";
   if (%state $= "cached")
      JoinScreenDetail.setText("The server's game mode, already on this computer.");
   else if (%offer)
      JoinScreenDetail.setText("The server's game mode: " @ %files @ ", " @ sizeText($Join::Bytes) @ " to download.");
   else
      JoinScreenDetail.setText("");
   JoinBarTrack.setVisible(%offer);
   %done = $Join::Bytes > 0 ? $Join::Received / $Join::Bytes : 1;
   JoinBar.setValue(%state $= "downloading" ? %done : 1);
   if (%state $= "connecting")
      %status = "Connecting...";
   else if (%state $= "accepted")
      %status = "Connected. Asking the server which mods it plays...";
   else if (%state $= "downloading")
      %status = "Downloading: " @ sizeText($Join::Received) @ " of " @ sizeText($Join::Bytes);
   else if (%state $= "cached")
      %status = "Nothing to download. Joining the match...";
   else if (%state $= "joining")
      %status = "Downloaded. Joining the match...";
   else if (%state $= "failed")
      %status = $Join::Why;
   else
      %status = "";
   JoinScreenStatus.setText(%status);
   JoinScreenBack.setText(%state $= "failed" ? "Back" : "Cancel");
}

function showJoinScreen()
{
   PlayMap.pop();
   closePause();
   Canvas.setContent(JoinScreenGui);
   Canvas.cursorOn();
   updateJoinScreen();
}

// The match is on: the board over the world, the keys steer.
function showMatch()
{
   setJoinState("playing");
   Canvas.setContent(MatchGui);
   PlayMap.push();
}

// The board's text, each with a dark copy behind it (the same font, a little down and right) so it reads over the pitch and the
// sky alike: no band behind it.
new GuiControlProfile(ClearProfile) { opaque = false; };
new GuiControlProfile(BlueScoreProfile) { fontType = "Arial"; fontSize = 36; fontColors[0] = "80 140 255 255"; justify = "center"; };
new GuiControlProfile(RedScoreProfile) { fontType = "Arial"; fontSize = 36; fontColors[0] = "255 118 104 255"; justify = "center"; };
new GuiControlProfile(DashProfile) { fontType = "Arial"; fontSize = 36; fontColors[0] = "255 255 255 255"; justify = "center"; };
new GuiControlProfile(ScoreShadow) { fontType = "Arial"; fontSize = 36; fontColors[0] = "0 0 0 230"; justify = "center"; };
new GuiControlProfile(ClockProfile) { fontType = "Arial"; fontSize = 22; fontColors[0] = "255 255 255 255"; justify = "center"; };
new GuiControlProfile(ClockShadow) { fontType = "Arial"; fontSize = 22; fontColors[0] = "0 0 0 230"; justify = "center"; };
new GuiControlProfile(NoteProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "255 236 180 255"; justify = "center"; };
new GuiControlProfile(NoteShadow) { fontType = "Arial"; fontSize = 16; fontColors[0] = "0 0 0 230"; justify = "center"; };
new GuiControlProfile(BannerProfile) { fontType = "Arial"; fontSize = 48; fontColors[0] = "255 236 180 255"; justify = "center"; };
new GuiControlProfile(BannerShadow) { fontType = "Arial"; fontSize = 48; fontColors[0] = "0 0 0 230"; justify = "center"; };
new GuiControlProfile(TeamProfile) { fontType = "Arial"; fontSize = 15; fontColors[0] = "235 238 245 255"; justify = "left"; };
new GuiControlProfile(TeamShadow) { fontType = "Arial"; fontSize = 15; fontColors[0] = "0 0 0 230"; justify = "left"; };

// Sound on: the kick plays through the engine's mixer. Nothing opens a sound card for it, so the headless demo is silent.
OpenALInitDriver();

// The pitch's reach (src/assets.rs holds these numbers; `the_mods_scripts_agree_with_the_pitchs_numbers` checks them): a sound
// the server places is kept inside it.
$Pitch::HalfWidth = 8;
$Pitch::HalfLength = 12;
$Pitch::GoalDepth = 1.5;
$Pitch::GoalHeight = 2.1;

// The one sound the server may ask for: the kick, played at a point on the pitch as a 3D sound, quieter farther from the
// listener. Any other file plays nothing, and a place that is not three numbers plays nothing; one past the pitch is moved
// onto its edge.
new AudioDescription(ServerSound3D)
{
   volume = 0.8;
   isLooping = false;
   is3D = true;
   referenceDistance = 4;
   maxDistance = 40;
};

// Whether %text is a plain number (digits, a point, a sign, an exponent; nothing else, and not too long).
function isPlainNumber(%text)
{
   return %text !$= "" && strlen(%text) <= 24 && stripChars(%text, "0123456789.-+e") $= "";
}

// %value as a number, kept between %low and %high.
function clampNumber(%value, %low, %high)
{
   %value = %value + 0;
   return %value < %low ? %low : (%value > %high ? %high : %value);
}

function clientCmdPlaySound(%file, %x, %y, %z)
{
   if (strcmp(%file, "sounds/kick.wav") != 0 || !isPlainNumber(%x) || !isPlainNumber(%y) || !isPlainNumber(%z))
      return;
   %x = clampNumber(%x, -$Pitch::HalfWidth, $Pitch::HalfWidth);
   %y = clampNumber(%y, -($Pitch::HalfLength + $Pitch::GoalDepth), $Pitch::HalfLength + $Pitch::GoalDepth);
   %z = clampNumber(%z, 0, $Pitch::GoalHeight);
   %handle = alxCreateSource(ServerSound3D, %file, %x, %y, %z);
   $Sound::Last = %handle ? alxPlay(%handle) : 0;
   $Sound::At = %x SPC %y SPC %z;
   $Sound::Played++;
}

// The team this client plays for, and the way it attacks (1: toward +y, -1: toward -y). Its own goals come first on the board,
// in its color.
function clientCmdSoccerTeam(%name, %attack)
{
   $Match::Team = %name;
   $Match::Attack = %attack;
   MatchMine.setProfile(%name $= "Red" ? RedScoreProfile : BlueScoreProfile);
   MatchTheirs.setProfile(%name $= "Red" ? BlueScoreProfile : RedScoreProfile);
   setShadowText(MatchTeam, "You play for" SPC %name);
}

// Fields of the server's text, separated by tabs: the round, the phase (play, goal or over), the seconds left, Blue's goals,
// Red's goals and a line to show. A goal and the end of a round are shown in the middle of the screen for as long as their
// pause lasts; the other lines go small under the clock.
function clientCmdSoccerState(%state)
{
   $Match::State = %state;
   $Match::Updates++;
   if ($Join::State !$= "playing")
      showMatch();
   $Match::Phase = getField(%state, 1);
   %left = getField(%state, 2);
   %mine = getField(%state, $Match::Team $= "Red" ? 4 : 3);
   %theirs = getField(%state, $Match::Team $= "Red" ? 3 : 4);
   setShadowText(MatchMine, %mine);
   setShadowText(MatchDash, "-");
   setShadowText(MatchTheirs, %theirs);
   %message = getField(%state, 5);
   %seconds = %left % 60;
   %clock = mFloor(%left / 60) @ ":" @ (%seconds < 10 ? "0" : "") @ %seconds;
   setShadowText(MatchClock, $Match::Phase $= "over" ? "" : %clock);
   setShadowText(MatchNote, $Match::Phase $= "play" ? %message : "");
   setShadowText(MatchBanner, $Match::Phase $= "play" ? "" : %message);
   setShadowText(MatchBannerSub, $Match::Phase $= "over" ? "Next round in" SPC %left @ "s" : "");
}

// A text control %name and its shadow (%name @ "Shadow", in %shadow, drawn first) in %parent, at %x %y, %w by %h.
function addShadowText(%parent, %name, %profile, %shadow, %x, %y, %w, %h)
{
   %back = new GuiTextCtrl(%name @ "Shadow")
   {
      profile = %shadow; position = (%x + 2) SPC (%y + 2); extent = %w SPC %h; visible = true; enabled = true; text = "";
   };
   %parent.add(%back);
   %front = new GuiTextCtrl(%name)
   {
      profile = %profile; position = %x SPC %y; extent = %w SPC %h; visible = true; enabled = true; text = "";
   };
   %parent.add(%front);
}

function setShadowText(%name, %text)
{
   %name.setText(%text);
   %shadow = %name @ "Shadow";
   %shadow.setText(%text);
}

// ---- the menus' look

new GuiControlProfile(MenuBackProfile) { opaque = true; fillColor = "22 40 30 255"; };
new GuiControlProfile(MenuPanelProfile) { opaque = true; border = true; fillColor = "28 48 38 240"; borderColor = "86 128 98 255"; };
new GuiControlProfile(MenuTitleProfile) { fontType = "Arial"; fontSize = 48; fontColors[0] = "246 244 232 255"; };
new GuiControlProfile(MenuHeadProfile) { fontType = "Arial"; fontSize = 30; fontColors[0] = "246 244 232 255"; justify = "center"; };
new GuiControlProfile(MenuModProfile) { fontType = "Arial"; fontSize = 24; fontColors[0] = "255 214 120 255"; justify = "center"; };
new GuiControlProfile(MenuTextProfile) { fontType = "Arial"; fontSize = 18; fontColors[0] = "226 236 228 255"; };
new GuiControlProfile(MenuCenterProfile) { fontType = "Arial"; fontSize = 18; fontColors[0] = "226 236 228 255"; justify = "center"; };
new GuiControlProfile(MenuDimProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "160 184 166 255"; };
new GuiControlProfile(MenuDimCenterProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "160 184 166 255"; justify = "center"; };
new GuiControlProfile(MenuNoteProfile) { fontType = "Arial"; fontSize = 20; fontColors[0] = "255 214 120 255"; justify = "center"; };
new GuiControlProfile(MenuTrackProfile) { opaque = true; border = true; fillColor = "14 24 18 255"; borderColor = "86 128 98 255"; };
new GuiControlProfile(MenuBarProfile) { fillColor = "120 200 120 255"; };
new GuiControlProfile(MenuListProfile)
{
   fontType = "Arial";
   fontSize = 18;
   fontColors[0] = "226 236 228 255";
   fontColors[1] = "255 214 120 255";
   fontColorHL = "255 214 120 255";
   fillColorHL = "54 100 70 255";
   opaque = false;
   canKeyFocus = true;
};
new GuiControlProfile(MenuScrollProfile) { opaque = true; border = true; fillColor = "16 28 20 255"; borderColor = "86 128 98 255"; };
new GuiControlProfile(MenuEditProfile)
{
   opaque = true;
   border = true;
   fillColor = "16 28 20 230";
   fillColorHL = "70 120 86 255";
   borderColor = "255 200 100 255";
   fontType = "Arial";
   fontSize = 18;
   fontColors[0] = "240 246 240 255";
   fontColorHL = "255 255 255 255";
   cursorColor = "255 214 120 255";
   canKeyFocus = true;
   autoSizeHeight = false;
};
new GuiControlProfile(MenuButtonProfile)
{
   opaque = true;
   border = true;
   fillColor = "46 78 58 255";
   fillColorHL = "255 190 80 255";
   fillColorNA = "30 40 34 255";
   borderColor = "100 150 114 255";
   fontType = "Arial";
   fontSize = 22;
   fontColors[0] = "240 246 240 255";
   fontColors[1] = "20 30 24 255";
   fontColors[2] = "120 136 126 255";
   justify = "center";
   canKeyFocus = true;
};

createCanvas("torque-example-mods-demo");

// ---- the main menu

new GuiControl(MainMenuGui)
{
   profile = MenuBackProfile; visible = true; enabled = true; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(MainPanel)
   {
      profile = ClearProfile; visible = true; enabled = true; position = "40 50"; extent = "720 500"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(MainTitle) { profile = MenuTitleProfile; visible = true; enabled = true; position = "20 20"; extent = "660 56"; text = "Soccer"; };
      new GuiTextCtrl(MainLine)
      {
         profile = MenuDimProfile; visible = true; enabled = true; position = "22 80"; extent = "660 20";
         text = "One player a team, one ball. The server sends you its game mode when you join.";
      };
      new GuiTextCtrl(NameLabel) { profile = MenuTextProfile; visible = true; enabled = true; position = "22 136"; extent = "110 24"; text = "Your name"; };
      new GuiTextEditCtrl(NameEdit)
      {
         profile = MenuEditProfile; visible = true; enabled = true; position = "140 132"; extent = "240 30"; maxLength = 24; text = "";
         altCommand = "saveName();";
      };
      new GuiButtonCtrl(HostButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "20 200"; extent = "280 56"; text = "Host a match"; command = "showHost();";
      };
      new GuiButtonCtrl(JoinButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "20 272"; extent = "280 56"; text = "Join a match"; command = "showJoin();";
      };
      new GuiButtonCtrl(QuitButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "20 344"; extent = "280 56"; text = "Quit"; command = "quitGame();";
      };
      new GuiTextCtrl(HostNote)
      {
         profile = MenuDimProfile; visible = true; enabled = true; position = "322 218"; extent = "380 20"; text = "Start a match here. Others can join it.";
      };
      new GuiTextCtrl(JoinNote)
      {
         profile = MenuDimProfile; visible = true; enabled = true; position = "322 290"; extent = "380 20"; text = "Find a match on your network, or type an address.";
      };
      new GuiTextCtrl(MainStatus)
      {
         profile = MenuNoteProfile; visible = true; enabled = true; position = "20 450"; extent = "680 28"; text = "";
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

function showMainMenu()
{
   PlayMap.pop();
   closePause();
   NameEdit.setValue($Pref::Player::Name);
   Canvas.setContent(MainMenuGui);
   Canvas.cursorOn();
   if ($Game::JoinOnly)
   {
      HostButton.setVisible(0);
      HostNote.setVisible(0);
      QuitButton.setVisible(0);
      JoinButton.setText("Join the match");
      JoinButton.command = "joinPage();";
      JoinNote.setText("Join the match on the server this page came from.");
   }
}

// A browser's Join: the host opens a new connection to the page's server and joins it.
function joinPage()
{
   saveName();
   MainStatus.setText("");
   $Game::JoinAsked = 1;
}

function quitGame()
{
   $Game::QuitAsked = 1;
   quit();
}

// ---- who may join a match hosted here

new GuiControl(HostGui)
{
   profile = MenuBackProfile; visible = true; enabled = true; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(HostPanel)
   {
      profile = ClearProfile; visible = true; enabled = true; position = "40 50"; extent = "720 500"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(HostTitle) { profile = MenuTitleProfile; visible = true; enabled = true; position = "20 20"; extent = "660 56"; text = "Host a match"; };
      new GuiTextCtrl(HostLine) { profile = MenuDimProfile; visible = true; enabled = true; position = "22 80"; extent = "660 20"; text = "Who can join the match you start?"; };
      new GuiButtonCtrl(HostLanButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "20 140"; extent = "280 56"; text = "LAN only"; command = "hostGame(\"lan\");";
      };
      new GuiTextCtrl(HostLanNote) { profile = MenuDimProfile; visible = true; enabled = true; position = "322 158"; extent = "380 20"; text = "Players on your own network can join."; };
      new GuiButtonCtrl(HostPublicButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "20 212"; extent = "280 56"; text = "Public"; command = "hostGame(\"public\");";
      };
      new GuiTextCtrl(HostPublicNote)
      {
         profile = MenuDimProfile; visible = true; enabled = true; position = "322 222"; extent = "380 20"; text = "Anyone who can reach this computer can join.";
      };
      new GuiTextCtrl(HostPublicNote2)
      {
         profile = MenuDimProfile; visible = true; enabled = true; position = "322 244"; extent = "380 20"; text = "Needs UDP 28000 forwarded to this computer.";
      };
      new GuiButtonCtrl(HostBackButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "20 284"; extent = "280 56"; text = "Back"; command = "showMainMenu();";
      };
      new GuiTextCtrl(HostStatus) { profile = MenuNoteProfile; visible = true; enabled = true; position = "20 450"; extent = "680 28"; text = ""; };
   };
};

function showHost()
{
   saveName();
   HostStatus.setText("");
   Canvas.setContent(HostGui);
}

// %scope is "lan" or "public".
function hostGame(%scope)
{
   $Game::HostScope = %scope $= "public" ? "public" : "lan";
   saveName();
   HostStatus.setText("Starting a match...");
   $Game::HostAsked = 1;
}

// ---- the network's matches

new GuiControl(JoinGui)
{
   profile = MenuBackProfile; visible = true; enabled = true; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(JoinPanel)
   {
      profile = ClearProfile; visible = true; enabled = true; position = "20 30"; extent = "760 540"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(JoinTitle) { profile = MenuTitleProfile; visible = true; enabled = true; position = "0 0"; extent = "700 56"; text = "Join a match"; };
      new GuiTextCtrl(JoinHeaderGame) { profile = MenuDimProfile; visible = true; enabled = true; position = "4 74"; extent = "200 20"; text = "Server"; };
      new GuiTextCtrl(JoinHeaderPlayers) { profile = MenuDimProfile; visible = true; enabled = true; position = "404 74"; extent = "90 20"; text = "Players"; };
      new GuiTextCtrl(JoinHeaderBots) { profile = MenuDimProfile; visible = true; enabled = true; position = "504 74"; extent = "90 20"; text = "Bots"; };
      new GuiTextCtrl(JoinHeaderPing) { profile = MenuDimProfile; visible = true; enabled = true; position = "604 74"; extent = "80 20"; text = "Ping"; };
      new GuiScrollCtrl(ServerScroll)
      {
         profile = MenuScrollProfile; visible = true; enabled = true; position = "0 98"; extent = "760 252"; hScrollBar = "alwaysOff"; vScrollBar = "dynamic";
         new GuiTextListCtrl(ServerList)
         {
            profile = MenuListProfile; visible = true; enabled = true; position = "2 2"; extent = "740 20"; columns = "0 400 500 600";
         };
      };
      new GuiTextCtrl(AddressLabel) { profile = MenuTextProfile; visible = true; enabled = true; position = "4 372"; extent = "76 24"; text = "Address"; };
      new GuiTextEditCtrl(AddressEdit)
      {
         profile = MenuEditProfile; visible = true; enabled = true; position = "82 368"; extent = "498 32"; maxLength = 260; text = "";
         placeholderText = "127.0.0.1:28000"; altCommand = "connectAddress();";
      };
      new GuiButtonCtrl(ConnectButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "590 364"; extent = "170 40"; text = "Connect"; command = "connectAddress();";
      };
      new GuiButtonCtrl(RefreshButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "0 440"; extent = "200 50"; text = "Refresh"; command = "refreshServers();";
      };
      new GuiButtonCtrl(JoinSelectedButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "280 440"; extent = "200 50"; text = "Join"; command = "joinSelected();";
      };
      new GuiButtonCtrl(BackButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "560 440"; extent = "200 50"; text = "Back"; command = "showMainMenu();";
      };
      new GuiTextCtrl(JoinStatus) { profile = MenuNoteProfile; visible = true; enabled = true; position = "0 506"; extent = "760 28"; text = ""; };
   };
};

function showJoin()
{
   saveName();
   AddressEdit.setValue($Pref::Client::LastAddress);
   Canvas.setContent(JoinGui);
   refreshServers();
}

function refreshServers()
{
   ServerList.clear();
   $Game::ServerRows = 0;
   JoinStatus.setText("Looking for matches...");
   queryLanServers($Game::Port, 0, "", "", 0, 100, 0, 2, 0, 100, 0);
}

function onServerQueryStatus(%status, %message, %value)
{
   if (%status !$= "done")
      return;
   fillServerList();
   JoinStatus.setText(getServerCount() == 0 ? "No matches found. Host one, or refresh." : "Pick a match and press Join.");
}

function fillServerList()
{
   ServerList.clear();
   for (%i = 0; %i < getServerCount(); %i++)
   {
      if (!setServerInfo(%i))
         continue;
      %players = $ServerInfo::PlayerCount @ " / " @ $ServerInfo::MaxPlayers;
      ServerList.addRow(%i, $ServerInfo::Name TAB %players TAB $ServerInfo::BotCount TAB $ServerInfo::Ping);
   }
   $Game::ServerRows = ServerList.rowCount();
   if ($Game::ServerRows > 0)
      ServerList.setSelectedRow(0);
}

function joinSelected()
{
   %row = ServerList.getSelectedId();
   if (%row $= "" || %row < 0 || !setServerInfo(%row))
   {
      JoinStatus.setText("Pick a match first.");
      return;
   }
   cancelJoinLookup();
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
      connectTo(%address);
}

// ---- joining a server: its mods' download, then the match

new GuiControl(JoinScreenGui)
{
   profile = MenuBackProfile; visible = true; enabled = true; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(JoinScreenPanel)
   {
      profile = MenuPanelProfile; visible = true; enabled = true; position = "120 160"; extent = "560 280"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(JoinScreenTitle) { profile = MenuHeadProfile; visible = true; enabled = true; position = "20 22"; extent = "520 38"; text = ""; };
      new GuiTextCtrl(JoinScreenMod) { profile = MenuModProfile; visible = true; enabled = true; position = "20 76"; extent = "520 30"; text = ""; };
      new GuiTextCtrl(JoinScreenDetail) { profile = MenuDimCenterProfile; visible = true; enabled = true; position = "20 110"; extent = "520 22"; text = ""; };
      new GuiControl(JoinBarTrack)
      {
         profile = MenuTrackProfile; visible = true; enabled = true; position = "40 146"; extent = "480 24";
         new GuiProgressCtrl(JoinBar) { profile = MenuBarProfile; visible = true; enabled = true; position = "2 2"; extent = "476 20"; };
      };
      new GuiTextCtrl(JoinScreenStatus) { profile = MenuCenterProfile; visible = true; enabled = true; position = "20 182"; extent = "520 24"; text = ""; };
      new GuiButtonCtrl(JoinScreenBack)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "190 220"; extent = "180 44"; text = "Cancel"; command = "leaveMatch();";
      };
   };
};

// ---- the match

// The board: the score and the clock at the top middle of the screen with a line of news under them, the team in the top left
// corner, the keys in the bottom left, and the goals and the round's end in the middle.
new GameTSCtrl(MatchGui)
{
   position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(MatchTop) { profile = ClearProfile; visible = true; enabled = true; position = "200 0"; extent = "400 104"; horizSizing = "center"; };
   new GuiControl(MatchCenter)
   {
      profile = ClearProfile; visible = true; enabled = true; position = "100 230"; extent = "600 100"; horizSizing = "center"; vertSizing = "center";
   };
   new GuiControl(MatchCorner) { profile = ClearProfile; visible = true; enabled = true; position = "0 0"; extent = "300 32"; };
   new GuiControl(MatchKeys) { profile = ClearProfile; visible = true; enabled = true; position = "0 568"; extent = "500 32"; vertSizing = "top"; };
   new GuiControl(PausePanel)
   {
      profile = MenuPanelProfile; position = "250 190"; extent = "300 220"; horizSizing = "center"; vertSizing = "center";
      visible = false;
      new GuiTextCtrl(PauseTitle) { profile = MenuHeadProfile; visible = true; enabled = true; position = "10 18"; extent = "280 38"; text = "Paused"; };
      new GuiTextCtrl(PauseNote) { profile = MenuDimCenterProfile; visible = true; enabled = true; position = "10 58"; extent = "280 20"; text = "The match goes on without you."; };
      new GuiButtonCtrl(PauseBack)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "40 92"; extent = "220 48"; text = "Back to the match"; command = "closePause();";
      };
      new GuiButtonCtrl(PauseLeave)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "40 152"; extent = "220 48"; text = "Leave the match"; command = "leaveMatch();";
      };
   };
};

function buildBoard()
{
   addShadowText(MatchTop, MatchMine, BlueScoreProfile, ScoreShadow, 115, 4, 70, 44);
   addShadowText(MatchTop, MatchDash, DashProfile, ScoreShadow, 185, 4, 30, 44);
   addShadowText(MatchTop, MatchTheirs, RedScoreProfile, ScoreShadow, 215, 4, 70, 44);
   addShadowText(MatchTop, MatchClock, ClockProfile, ClockShadow, 0, 48, 400, 28);
   addShadowText(MatchTop, MatchNote, NoteProfile, NoteShadow, 0, 78, 400, 22);
   addShadowText(MatchCenter, MatchBanner, BannerProfile, BannerShadow, 0, 0, 600, 58);
   addShadowText(MatchCenter, MatchBannerSub, ClockProfile, ClockShadow, 0, 62, 600, 30);
   addShadowText(MatchCorner, MatchTeam, TeamProfile, TeamShadow, 14, 8, 280, 20);
   addShadowText(MatchKeys, MatchKeyHelp, TeamProfile, TeamShadow, 14, 6, 480, 20);
   setShadowText(MatchKeyHelp, "W A S D run   Esc menu");
}

// ---- the keys

// The camera looks across the pitch from its side with the player's team attacking to the right, and a player always faces
// the goal it attacks: so D (right on the screen) is the player's forward and W (away from the camera) its left, whichever
// team it plays for.
new ActionMap(PlayMap);
function applyMoves()
{
   $mvForwardAction = $Game::KeyRight;
   $mvBackwardAction = $Game::KeyLeft;
   $mvLeftAction = $Game::KeyUp;
   $mvRightAction = $Game::KeyDown;
}
function releaseKeys()
{
   $Game::KeyUp = 0;
   $Game::KeyDown = 0;
   $Game::KeyLeft = 0;
   $Game::KeyRight = 0;
   applyMoves();
}
function moveUp(%v) { $Game::KeyUp = %v != 0; applyMoves(); }
function moveDown(%v) { $Game::KeyDown = %v != 0; applyMoves(); }
function moveLeft(%v) { $Game::KeyLeft = %v != 0; applyMoves(); }
function moveRight(%v) { $Game::KeyRight = %v != 0; applyMoves(); }
function pauseKey(%v) { if (%v) openPause(); }
PlayMap.bind(keyboard, w, moveUp);
PlayMap.bind(keyboard, s, moveDown);
PlayMap.bind(keyboard, a, moveLeft);
PlayMap.bind(keyboard, d, moveRight);
PlayMap.bind(keyboard, escape, pauseKey);

// The Esc menu: the player stands while it is open (the match goes on); Esc again closes it.
new ActionMap(PauseMap);
function unpauseKey(%v) { if (%v) closePause(); }
PauseMap.bind(keyboard, escape, unpauseKey);

function openPause()
{
   releaseKeys();
   PlayMap.pop();
   PauseMap.push();
   PausePanel.setVisible(1);
}

function closePause()
{
   if (!PausePanel.isVisible())
      return;
   PausePanel.setVisible(0);
   PauseMap.pop();
   if ($Join::State $= "playing")
      PlayMap.push();
}

buildBoard();
showMainMenu();
