// The demo's client: the main menu (host a match here, join one from the network's list of games or by address: the examples'
// common/matches.cs, with the screen that shows the server's mods downloading), the match as the server sends it with a small
// board over the world, the keys and the Esc menu. The client has none of the game mode's rules: it is told its team, the score and the clock, and shows them.
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

// ---- joining (the screens are common/matches.cs's)

// Joins the server at %address; the join screen names it %shown when given.
function connectTo(%address, %shown)
{
   $Match::Team = "";
   $Match::Updates = "";
   joinServer(%address, %shown);
}

function disconnect()
{
   if (isObject(ServerConnection))
      ServerConnection.delete();
}

function GameConnection::onConnectionAccepted(%this)
{
   joinAccepted();
}

function GameConnection::onConnectRequestRejected(%this, %why)
{
   $Why = %why;
   joinRejected(%why);
}

function GameConnection::onConnectRequestTimedOut(%this)
{
   joinTimedOut();
}

function GameConnection::onConnectionDropped(%this, %reason)
{
   joinDropped("The server closed the connection: " @ %reason);
}

function GameConnection::onConnectionTimedOut(%this)
{
   joinDropped("Lost the connection to the server.");
}

// The Esc menu's Leave: out of the server, back to the menu.
function leaveMatch()
{
   leaveJoin("");
}

// The join screen's hooks (common/matches.cs).
function joinScreenShown()
{
   PlayMap.pop();
   closePause();
}

function joinLeft(%why)
{
   releaseKeys();
   closePause();
   $Game::LeaveAsked = 1;
   showMainMenu();
   if (%why !$= "")
      MainStatus.setText(%why);
}

// The match is on: the board over the world, the keys steer.
function showMatch()
{
   joinPlaying();
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

// Sound on: the kick (the server plays it in the kicking player's sound slot, as part of that player's update) plays
// through the engine's mixer at the player, from the mod's sound file. A window opens the sound card for it; headless, the
// mixer runs on the clock and nothing is heard.
OpenALInitDriver();

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
new GuiControlProfile(MenuModProfile) { fontType = "Arial"; fontSize = 24; fontColors[0] = "255 214 120 255"; justify = "right"; };
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

// Hosting and joining (the list of matches, the address box, the download screen); the join screen calls the mod the game mode.
$Join::ModsLabel = "The server's game mode";
exec("common/matches.cs");

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
      profile = MenuPanelProfile; position = "250 222"; extent = "300 156"; horizSizing = "center"; vertSizing = "center";
      visible = false;
      new GuiButtonCtrl(PauseBack)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "40 24"; extent = "220 48"; text = "Continue"; command = "closePause();";
      };
      new GuiButtonCtrl(PauseLeave)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "40 84"; extent = "220 48"; text = "Leave"; command = "leaveMatch();";
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
