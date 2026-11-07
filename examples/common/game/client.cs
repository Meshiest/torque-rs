// What the example games' clients share: the way to a server and back, a note in the middle of the view and the readout in
// its corner. A game's client.cs execs this first; its view has a GuiTextCtrl named PlayNote and one named PlayStats.

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

// ---- the corner readout
//
// A line at the top right of the view, as "60 fps  32 ms", four times a second: the frames a second the host counted
// ($Client::FrameRate) and the connection's round trip (getPing). A client that runs the server it plays on
// ($Client::HostsServer: a game played alone or hosted here) has no network between them, so its line shows the frames alone.
new GuiControlProfile(StatsProfile) { fontType = "Arial"; fontSize = 14; fontColors[0] = "176 186 204 255"; justify = "right"; };

function statsText()
{
   %text = "";
   if ($Client::FrameRate !$= "")
      %text = $Client::FrameRate @ " fps";
   if (isObject(ServerConnection) && !$Client::HostsServer && ServerConnection.isConnected())
      %text = %text @ (%text $= "" ? "" : "  ") @ ServerConnection.getPing() @ " ms";
   return %text;
}

function updateStats()
{
   cancel($Client::StatsTimer);
   if (isObject(PlayStats))
      PlayStats.setText(statsText());
   $Client::StatsTimer = schedule(250, 0, "updateStats");
}
updateStats();

// ---- the figure's poses
//
// Ctrl held crouches, C crouches or stands (a browser closes its tab on Ctrl+W), Z lies down or gets up, and Space while
// crouched by C or lying down stands up rather than jumping. Each pose is a move trigger the server reads as Torque3D names
// them ($player::crouchTrigger 3 and $player::proneTrigger 1, set in common/server.cs), held while its count is odd. The
// server refuses a pose with no room for it (no standing up under a low ceiling) and shows the others the pose it chose. (The
// figure walks by default and Shift sprints: each game's own moves.)
$Pose::CrouchTrigger = 3;
$Pose::ProneTrigger = 1;
function setTriggerHeld(%trigger, %held)
{
   %count = $mvTriggerCount[%trigger];
   if ((%count % 2 == 1) != (%held != 0))
      $mvTriggerCount[%trigger] = %count + 1;
}
function poseProne() { return $mvTriggerCount[$Pose::ProneTrigger] % 2 == 1; }
// The crouch trigger: held while Ctrl is or while C left the figure crouched.
function holdCrouch() { setTriggerHeld($Pose::CrouchTrigger, $Pose::CrouchHeld || $Pose::CrouchToggled); }
// Ctrl: crouched while held; pressed lying down, the figure gets up to its knees.
function crouchKey(%v)
{
   if (%v && poseProne())
      setTriggerHeld($Pose::ProneTrigger, false);
   $Pose::CrouchHeld = %v != 0;
   holdCrouch();
}
// C: crouches or stands; lying down, it gets up to a crouch.
function crouchToggleKey(%v)
{
   if (!%v)
      return;
   if (poseProne())
   {
      setTriggerHeld($Pose::ProneTrigger, false);
      $Pose::CrouchToggled = true;
   }
   else
      $Pose::CrouchToggled = !$Pose::CrouchToggled;
   holdCrouch();
}
// Z: lies down or gets up.
function proneKey(%v)
{
   if (!%v)
      return;
   %down = !poseProne();
   setTriggerHeld($Pose::ProneTrigger, %down);
   if (%down)
      $Pose::CrouchToggled = false;
   holdCrouch();
}
// Space lying down or crouched by C stands up rather than jumping: the press lets go of the pose and its release is eaten
// too, so the jump trigger's count stays even. True when the key was the pose's (the game's jump leaves its trigger alone).
function poseJumpKey(%v)
{
   if (%v && (poseProne() || $Pose::CrouchToggled))
   {
      setTriggerHeld($Pose::ProneTrigger, false);
      $Pose::CrouchToggled = false;
      holdCrouch();
      $Pose::JumpEaten = true;
      return true;
   }
   if (!%v && $Pose::JumpEaten)
   {
      $Pose::JumpEaten = false;
      return true;
   }
   return false;
}
// Lets go of every pose (a map change, a seat or a respawn leaves none held: the figure stands up when there is room).
function releasePoseKeys()
{
   $Pose::CrouchHeld = false;
   $Pose::CrouchToggled = false;
   setTriggerHeld($Pose::CrouchTrigger, false);
   setTriggerHeld($Pose::ProneTrigger, false);
}
// Lets go of the poses a key holds while it is down (Ctrl's crouch), the ones C and Z left kept: the keys are taken off the
// figure for a moment (the chat), and the key's release would not be seen.
function releaseHeldPoseKeys()
{
   $Pose::CrouchHeld = false;
   holdCrouch();
}
// Binds the pose keys in action map %map.
function bindPoseKeys(%map)
{
   %map.bind(keyboard, lcontrol, crouchKey);
   %map.bind(keyboard, c, crouchToggleKey);
   %map.bind(keyboard, z, proneKey);
}
