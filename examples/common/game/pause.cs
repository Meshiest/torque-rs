// The examples' pause menu: over the view (the game goes on: others still play), the cursor on and the game's keys off.
// Esc opens it in a game and closes it again; Resume goes back, Leave game goes to the game's main menu, Quit ends the
// program. $Client::Paused is 1 while it is up.
//
// A game's client.cs execs this after common/client.cs and gives the hooks: pauseAllowed() (whether Esc may pause now),
// onPause() (its keys taken off and let go), onResume() (its keys back), leaveGame() (back to its main menu: it calls
// closePause() first) and quitGame(). It binds Esc to pauseKey in its own maps.

new GuiControlProfile(PauseShadeProfile) { opaque = true; fillColor = "8 10 16 140"; };
new GuiControlProfile(PausePanelProfile) { opaque = true; border = true; fillColor = "30 36 48 235"; borderColor = "70 82 104 255"; };
new GuiControlProfile(PauseHeadProfile) { fontType = "Arial"; fontSize = 32; fontColors[0] = "246 240 228 255"; justify = "center"; };
new GuiControlProfile(PauseHintProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "150 160 178 255"; justify = "center"; };
new GuiControlProfile(PauseButtonProfile)
{
   opaque = true;
   border = true;
   fillColor = "48 58 78 255";
   fillColorHL = "255 176 72 255";
   fillColorNA = "30 34 42 255";
   borderColor = "100 118 150 255";
   fontType = "Arial";
   fontSize = 22;
   fontColors[0] = "238 242 248 255";
   fontColors[1] = "20 24 32 255";
   fontColors[2] = "120 126 138 255";
   justify = "center";
   canKeyFocus = true;
};

new GuiControl(PauseGui)
{
   profile = PauseShadeProfile; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(PausePanel)
   {
      profile = PausePanelProfile; position = "240 122"; extent = "320 356"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(PauseTitle)
      {
         profile = PauseHeadProfile; position = "0 26"; extent = "320 40"; visible = true; enabled = true;
         text = "Paused";
      };
      new GuiButtonCtrl(ResumeButton)
      {
         profile = PauseButtonProfile; position = "40 92"; extent = "240 56"; visible = true; enabled = true;
         text = "Resume"; command = "resumeGame();";
      };
      new GuiButtonCtrl(LeaveButton)
      {
         profile = PauseButtonProfile; position = "40 168"; extent = "240 56"; visible = true; enabled = true;
         text = "Leave game"; command = "leaveGame();";
      };
      new GuiButtonCtrl(PauseQuitButton)
      {
         profile = PauseButtonProfile; position = "40 244"; extent = "240 56"; visible = true; enabled = true;
         text = "Quit"; command = "quitGame();";
      };
      new GuiTextCtrl(PauseNote)
      {
         profile = PauseHintProfile; position = "0 316"; extent = "320 20"; visible = true; enabled = true;
         text = "Esc: back to the game";
      };
   };
};

function pauseGame()
{
   if ($Client::Paused || !pauseAllowed())
      return;
   $Client::Paused = 1;
   onPause();
   Canvas.pushDialog(PauseGui);
   Canvas.cursorOn();
   PauseMap.push();
}

function resumeGame()
{
   if (!$Client::Paused)
      return;
   closePause();
   Canvas.cursorOff();
   onResume();
}

// The menu closed without going back to the game (leaving it, or the game's view replaced).
function closePause()
{
   if (!$Client::Paused)
      return;
   $Client::Paused = 0;
   PauseMap.pop();
   Canvas.popDialog(PauseGui);
}

function pauseKey(%v) { if (%v) pauseGame(); }
function resumeKey(%v) { if (%v) resumeGame(); }

// While paused, Esc goes back to the game.
new ActionMap(PauseMap);
PauseMap.bind(keyboard, escape, resumeKey);
