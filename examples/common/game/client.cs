// What the example games' clients share: the way to a server and back, and a note in the middle of the view. A game's
// client.cs execs this first; its view has a GuiTextCtrl named PlayNote.

// Connects to the server at %address ("a.b.c.d:port"), in place of any connection there is, as $Pref::Player::Name when the
// game names its player. A host's client passes %key, the secret its own server gave it (it marks the connection as the host's).
function connectTo(%address, %key)
{
   disconnect();
   $Client::MapCount = 0;
   %conn = new GameConnection(ServerConnection);
   if (%key !$= "")
      %conn.setConnectArgs($Pref::Player::Name, %key);
   else if ($Pref::Player::Name !$= "")
      %conn.setConnectArgs($Pref::Player::Name);
   %conn.connect(%address);
}

// Ends the connection, if there is one.
function disconnect()
{
   $Client::MapCount = 0;
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
