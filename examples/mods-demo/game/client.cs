// The demo's client: connectTo(%address) and disconnect(), the round's state as the server sends it, and a small board over the
// world. The client has none of the game mode's rules: it is told the round, the clock and the scores, and shows them.

exec("prefs.cs");

// Ask the server for a packet every tick, as large as the wire's rate record allows: the link is in memory, and the runners move
// fast enough for the default ten packets a second to show.
$pref::Net::PacketRateToClient = 32;
$pref::Net::PacketSize = 1023;

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

new GuiControlProfile(BandProfile) { opaque = true; fillColor = "12 16 24 170"; };
new GuiControlProfile(BoardProfile) { fontType = "Arial"; fontSize = 22; fontColors[0] = "255 255 255 255"; };
new GuiControlProfile(ScoreProfile) { fontType = "Arial"; fontSize = 18; fontColors[0] = "255 236 180 255"; };

// Fields of the server's text, separated by tabs: the round, the phase, the seconds left, then a line for each player.
function clientCmdOrbsState(%state)
{
   $Orbs::State = %state;
   $Orbs::Updates++;
   %round = getField(%state, 0);
   %phase = getField(%state, 1);
   %left = getField(%state, 2);
   if (%phase $= "play")
      OrbsClock.setText("Round" SPC %round SPC "-" SPC %left @ "s");
   else
      OrbsClock.setText("Round" SPC %round SPC "over - next round in" SPC %left @ "s");
   %lines = "";
   for (%i = 3; %i < getFieldCount(%state); %i++)
      %lines = %lines @ (%i > 3 ? "   " : "") @ getField(%state, %i);
   OrbsScores.setText(%lines);
}

function buildGui()
{
   createCanvas("torque-example-mods-demo");
   // The world's view, with a band of text over it.
   new GameTSCtrl(OrbsGui)
   {
      position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
      new GuiControl(OrbsBand)
      {
         profile = BandProfile; position = "0 0"; extent = "800 72"; horizSizing = "width";
         new GuiTextCtrl(OrbsClock) { profile = BoardProfile; position = "14 8"; extent = "560 28"; visible = true; enabled = true; text = "waiting for the server"; };
         new GuiTextCtrl(OrbsScores) { profile = ScoreProfile; position = "14 40"; extent = "760 24"; visible = true; enabled = true; text = ""; };
      };
   };
   Canvas.setContent(OrbsGui);
}

buildGui();
