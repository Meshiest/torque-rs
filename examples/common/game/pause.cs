// The examples' pause menu: over the view (the game goes on: others still play), the cursor on and the game's keys off.
// Esc opens it in a game and closes it again; Resume goes back, Leave game goes to the game's main menu, Quit ends the
// program. $Client::Paused is 1 while it is up.
//
// A game's client.cs execs this after common/client.cs and gives the hooks: pauseAllowed() (whether Esc may pause now),
// onPause() (its keys taken off and let go), onResume() (its keys back), leaveGame() (back to its main menu: it calls
// closePause() first) and quitGame(). It binds Esc to pauseKey in its own maps.
//
// A game with a map rotation lets its host pick the map: its server sends the host's client (and no other) the maps it may pick
// ('MapChoices', then a 'MapChoice' for each), and the menu then has a Change map entry that lists them and asks the server
// for the one chosen (changeMap: commandToServer). The server decides who the host is and which maps are allowed; a game
// that sends no maps has no such entry.

new GuiControlProfile(PauseShadeProfile) { opaque = true; fillColor = "8 10 16 140"; };
new GuiControlProfile(PausePanelProfile) { opaque = true; border = true; fillColor = "30 36 48 235"; borderColor = "70 82 104 255"; };
new GuiControlProfile(PauseHeadProfile) { fontType = "Arial"; fontSize = 32; fontColors[0] = "246 240 228 255"; justify = "center"; };
new GuiControlProfile(PauseHintProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "150 160 178 255"; justify = "center"; };
new GuiControlProfile(PauseScrollProfile) { opaque = true; border = true; fillColor = "16 20 28 255"; borderColor = "70 82 104 255"; };
new GuiControlProfile(PauseListProfile)
{
   fontType = "Arial";
   fontSize = 18;
   fontColors[0] = "226 230 238 255";
   fontColors[1] = "255 208 120 255";
   fontColorHL = "255 208 120 255";
   fillColorHL = "58 84 128 255";
   opaque = false;
   canKeyFocus = true;
};
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
      new GuiButtonCtrl(ChangeMapButton)
      {
         profile = PauseButtonProfile; position = "40 168"; extent = "240 56"; visible = false; enabled = true;
         text = "Change map"; command = "showMapPick();";
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
   // The maps the host may pick: a list to choose from, then Load map.
   new GuiControl(MapPanel)
   {
      profile = PausePanelProfile; position = "240 80"; extent = "320 440"; horizSizing = "center"; vertSizing = "center"; visible = false;
      new GuiTextCtrl(MapTitle)
      {
         profile = PauseHeadProfile; position = "0 26"; extent = "320 40"; visible = true; enabled = true;
         text = "Change map";
      };
      new GuiScrollCtrl(MapScroll)
      {
         profile = PauseScrollProfile; position = "40 84"; extent = "240 212"; visible = true; enabled = true;
         hScrollBar = "alwaysOff"; vScrollBar = "dynamic";
         new GuiTextListCtrl(MapList)
         {
            profile = PauseListProfile; position = "2 2"; extent = "220 20"; visible = true; enabled = true;
            columns = "0";
         };
      };
      new GuiButtonCtrl(MapLoadButton)
      {
         profile = PauseButtonProfile; position = "40 312"; extent = "240 48"; visible = true; enabled = true;
         text = "Load map"; command = "loadSelectedMap();";
      };
      new GuiButtonCtrl(MapBackButton)
      {
         profile = PauseButtonProfile; position = "40 372"; extent = "240 48"; visible = true; enabled = true;
         text = "Back"; command = "hideMapPick();";
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
   layoutPause();
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
   hideMapPick();
}

function pauseKey(%v) { if (%v) pauseGame(); }
function resumeKey(%v) { if (%v) resumeGame(); }

// While paused, Esc goes back to the game.
new ActionMap(PauseMap);
PauseMap.bind(keyboard, escape, resumeKey);

// ---- the host's map change

// The maps the server sent (none for anyone but the host of a game with a rotation): $Client::MapChoice0 and on.
$Client::MapCount = 0;

function clientCmdMapChoices(%count)
{
   $Client::MapCount = 0;
}

function clientCmdMapChoice(%name)
{
   $Client::MapChoice[$Client::MapCount] = %name;
   $Client::MapCount++;
   layoutPause();
}

// The menu with or without its Change map entry: the buttons below it move down, and the panel grows to hold them.
function layoutPause()
{
   %y = 168;
   %host = $Client::MapCount > 0;
   ChangeMapButton.setVisible(%host);
   if (%host)
      %y = 244;
   LeaveButton.resize(40, %y, 240, 56);
   PauseQuitButton.resize(40, %y + 76, 240, 56);
   PauseNote.resize(0, %y + 148, 320, 20);
   %h = %y + 188;
   %size = PauseGui.getExtent();
   PausePanel.resize(mFloor((getWord(%size, 0) - 320) / 2), mFloor((getWord(%size, 1) - %h) / 2), 320, %h);
}

function showMapPick()
{
   if (!$Client::Paused || $Client::MapCount < 1)
      return;
   MapList.clear();
   for (%i = 0; %i < $Client::MapCount; %i++)
      MapList.addRow(%i, fileBase($Client::MapChoice[%i]));
   MapList.setSelectedRow(0);
   PausePanel.setVisible(false);
   MapPanel.setVisible(true);
}

function hideMapPick()
{
   MapPanel.setVisible(false);
   PausePanel.setVisible(true);
}

// Asks the server for the map %name (one of the list it sent); only its host's ask is heard.
function changeMap(%name)
{
   if (isObject(ServerConnection))
      commandToServer('ChangeMap', %name);
}

// Load map: the chosen row's map, and back to the game to watch it load.
function loadSelectedMap()
{
   %row = MapList.getSelectedId();
   if (%row $= "" || %row < 0 || %row >= $Client::MapCount)
      return;
   changeMap($Client::MapChoice[%row]);
   resumeGame();
}
