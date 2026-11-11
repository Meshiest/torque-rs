// The client's script: the engine runs this file. The program calls connectTo() and disconnect(). The GUI is the game's name on
// a dark background.

echo("Playing" SPC $Game::Name);

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

// Called when the server starts sending its objects.
function onGhostAlwaysStarted(%count)
{
}

new GuiControlProfile(BackProfile) { opaque = true; fillColor = "24 28 36 255"; };
new GuiControlProfile(TextProfile) { fontType = "Arial"; fontSize = 24; fontColors[0] = "230 230 230 255"; };

createCanvas($Game::Name);
new GuiControl(GameGui)
{
   profile = BackProfile; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiTextCtrl(GameTitle) { profile = TextProfile; position = "16 16"; extent = "768 32"; text = $Game::Name; };
};
Canvas.setContent(GameGui);
