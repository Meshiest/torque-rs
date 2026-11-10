// The demo's client: connectTo(%address) and disconnect(), the match as the server sends it, and a small board over the world.
// The client has none of the game mode's rules: it is told its team, the score and the clock, and shows them. Its player is
// steered with the move globals ($mvForwardAction and the others), as keys would steer it.

exec("prefs.cs");

// Ask the server for a packet every tick, as large as the wire's rate record allows: the link is in memory, and the ball moves
// fast enough for the default ten packets a second to show.
$pref::Net::PacketRateToClient = 32;
$pref::Net::PacketSize = 1023;

// The client looks on from outside its player, so its own player is drawn like the other.
$firstPerson = 0;

function connectTo(%address)
{
   %conn = new GameConnection(ServerConnection);
   %conn.connect(%address);
}

function disconnect()
{
   if (isObject(ServerConnection))
      ServerConnection.delete();
}

function GameConnection::onConnectRequestRejected(%this, %why)
{
   $Why = %why;
}

function onModDownloadStarted(%mods, %bytes)
{
   $Download::Started = %mods SPC %bytes;
}

function onModDownloadDone()
{
   $Download::Done++;
}

function onModDownloadFailed(%why)
{
   $Download::Failed = %why;
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

// The board: the score and the clock at the top middle of the screen with a line of news under them, the team in the top left
// corner, and the goals and the round's end in the middle.
function buildGui()
{
   createCanvas("torque-example-mods-demo");
   new GameTSCtrl(MatchGui)
   {
      position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
      new GuiControl(MatchTop) { profile = ClearProfile; position = "200 0"; extent = "400 104"; horizSizing = "center"; };
      new GuiControl(MatchCenter)
      {
         profile = ClearProfile; position = "100 230"; extent = "600 100"; horizSizing = "center"; vertSizing = "center";
      };
      new GuiControl(MatchCorner) { profile = ClearProfile; position = "0 0"; extent = "300 32"; };
   };
   addShadowText(MatchTop, MatchMine, BlueScoreProfile, ScoreShadow, 115, 4, 70, 44);
   addShadowText(MatchTop, MatchDash, DashProfile, ScoreShadow, 185, 4, 30, 44);
   addShadowText(MatchTop, MatchTheirs, RedScoreProfile, ScoreShadow, 215, 4, 70, 44);
   addShadowText(MatchTop, MatchClock, ClockProfile, ClockShadow, 0, 48, 400, 28);
   addShadowText(MatchTop, MatchNote, NoteProfile, NoteShadow, 0, 78, 400, 22);
   addShadowText(MatchCenter, MatchBanner, BannerProfile, BannerShadow, 0, 0, 600, 58);
   addShadowText(MatchCenter, MatchBannerSub, ClockProfile, ClockShadow, 0, 62, 600, 30);
   addShadowText(MatchCorner, MatchTeam, TeamProfile, TeamShadow, 14, 8, 280, 20);
   setShadowText(MatchClock, "waiting for the server");
   Canvas.setContent(MatchGui);
}

buildGui();
