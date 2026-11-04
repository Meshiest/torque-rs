// What the example games' clients share: the way to a server and back, and a note in the middle of the view. A game's
// client.cs execs this first; its view has a GuiTextCtrl named PlayNote.

// Connects to the server at %address ("a.b.c.d:port"), in place of any connection there is, as $Pref::Player::Name when the
// game names its player.
function connectTo(%address)
{
   disconnect();
   %conn = new GameConnection(ServerConnection);
   if ($Pref::Player::Name !$= "")
      %conn.setConnectArgs($Pref::Player::Name);
   %conn.connect(%address);
}

// Ends the connection, if there is one.
function disconnect()
{
   if (isObject(ServerConnection))
      ServerConnection.delete();
}

// A line in the middle of the view for a few seconds.
function showNote(%text)
{
   cancel($Client::NoteTimer);
   PlayNote.setText(%text);
   $Client::NoteTimer = schedule(2500, 0, "clearNote");
}

function clearNote()
{
   PlayNote.setText("");
}
