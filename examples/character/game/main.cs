new GuiControlProfile(CharacterBandProfile) { opaque = true; fillColor = "12 16 24 150"; };
new GuiControlProfile(CharacterTextProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "235 238 245 255"; };
new GuiControlProfile(CharacterStateProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "255 214 110 255"; justify = "right"; };
createCanvas("torque-rs character");
// How far the view reaches: a light haze from 1,500 units out, nothing drawn past 3,000 (a mission's Sky gives these as its
// fogDistance and visibleDistance).
$Character::fogDistance = 1500;
$Character::visibleDistance = 3000;
new GameTSCtrl(CharacterView) {
    position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
    new GuiControl(CharacterBand) {
        profile = CharacterBandProfile; position = "0 0"; extent = "800 28"; horizSizing = "width";
        new GuiTextCtrl(CharacterHelp) {
            profile = CharacterTextProfile; position = "10 5"; extent = "700 18"; visible = true; enabled = true;
            text = "W A S D walk    Shift sprint    Ctrl crouch    C toggle crouch    Z prone    Space jump    F pistol    Mouse or Q / E orbit    Wheel zoom    Esc quit";
        };
        new GuiTextCtrl(CharacterState) {
            profile = CharacterStateProfile; position = "700 5"; extent = "90 18"; horizSizing = "left"; visible = true; enabled = true; text = "idle";
        };
    };
};
Canvas.setContent(CharacterView);
Canvas.cursorOff();

function charForward(%v) { $charForward = %v; }
function charBack(%v) { $charBack = %v; }
function charLeft(%v) { $charLeft = %v; }
function charRight(%v) { $charRight = %v; }
function charRun(%v) { $charRun = %v; }
// Space lying down or crouched by C stands up rather than jumping.
function charJump(%v)
{
   if (%v && ($charProne || $charCrouchToggled))
   {
      $charProne = false;
      $charCrouchToggled = false;
      charHoldCrouch();
   }
   else
      $charJump = %v;
}
// Ctrl held crouches; C crouches or stands; Z lies down or gets up. Ctrl or C lying down gets up to a crouch.
function charHoldCrouch() { $charCrouch = $charCrouchHeld || $charCrouchToggled; }
function charCrouch(%v)
{
   if (%v)
      $charProne = false;
   $charCrouchHeld = %v;
   charHoldCrouch();
}
function charCrouchC(%v)
{
   if (!%v)
      return;
   if ($charProne)
   {
      $charProne = false;
      $charCrouchToggled = true;
   }
   else
      $charCrouchToggled = !$charCrouchToggled;
   charHoldCrouch();
}
function charProne(%v)
{
   if (!%v)
      return;
   $charProne = !$charProne;
   if ($charProne)
      $charCrouchToggled = false;
   charHoldCrouch();
}
function charTurnLeft(%v) { $charTurnLeft = %v; }
function charTurnRight(%v) { $charTurnRight = %v; }
function charYaw(%v) { $charYaw += %v; }
function charPitch(%v) { $charPitch += %v; }
function charZoom(%v) { $charZoom += %v; }
function charPistol(%v) { if (%v) $charPistol = !$charPistol; }
function charQuit(%v) { if (%v) quit(); }
function resetCanvas() {}

new ActionMap(CharacterMap);
CharacterMap.bind(keyboard, w, charForward);
CharacterMap.bind(keyboard, s, charBack);
CharacterMap.bind(keyboard, a, charLeft);
CharacterMap.bind(keyboard, d, charRight);
CharacterMap.bind(keyboard, lshift, charRun);
CharacterMap.bind(keyboard, space, charJump);
CharacterMap.bind(keyboard, lcontrol, charCrouch);
CharacterMap.bind(keyboard, c, charCrouchC);
CharacterMap.bind(keyboard, z, charProne);
CharacterMap.bind(keyboard, q, charTurnLeft);
CharacterMap.bind(keyboard, e, charTurnRight);
CharacterMap.bind(keyboard, f, charPistol);
CharacterMap.bind(keyboard, escape, charQuit);
CharacterMap.bind(mouse, xaxis, charYaw);
CharacterMap.bind(mouse, yaxis, charPitch);
CharacterMap.bind(mouse, zaxis, charZoom);
CharacterMap.push();

function showState(%name) { CharacterState.setText(%name); }
