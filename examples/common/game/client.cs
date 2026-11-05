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

// ---- the figure's poses
//
// Ctrl held crouches and Z lies down or gets up again: each is a move trigger the server reads as Torque3D names them
// ($player::crouchTrigger 3 and $player::proneTrigger 1, set in common/server.cs), held while its count is odd. The server
// refuses a pose with no room for it (no standing up under a low ceiling) and shows the others the pose it chose. (The
// figure walks by default and Shift sprints: each game's own moves.)
$Pose::CrouchTrigger = 3;
$Pose::ProneTrigger = 1;
function setTriggerHeld(%trigger, %held)
{
   %count = $mvTriggerCount[%trigger];
   if ((%count % 2 == 1) != (%held != 0))
      $mvTriggerCount[%trigger] = %count + 1;
}
function crouchKey(%v) { setTriggerHeld($Pose::CrouchTrigger, %v); }
function proneKey(%v)
{
   if (%v)
      setTriggerHeld($Pose::ProneTrigger, $mvTriggerCount[$Pose::ProneTrigger] % 2 == 0);
}
// Lets go of both pose keys (a map change or a seat leaves neither held: the figure stands up when there is room).
function releasePoseKeys()
{
   setTriggerHeld($Pose::CrouchTrigger, false);
   setTriggerHeld($Pose::ProneTrigger, false);
}
// Binds the pose keys in action map %map.
function bindPoseKeys(%map)
{
   %map.bind(keyboard, lcontrol, crouchKey);
   %map.bind(keyboard, z, proneKey);
}
