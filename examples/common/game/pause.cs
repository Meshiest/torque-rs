// The examples' pause menu: over the view (the game goes on: others still play), the cursor on and the game's keys off.
// Esc opens it in a game and closes it again; Resume goes back, Controls lists the game's keys, Leave game goes to the game's
// main menu, Quit ends the program. $Client::Paused is 1 while it is up.
//
// A game's client.cs execs this after common/client.cs and gives the hooks: pauseAllowed() (whether Esc may pause now),
// onPause() (its keys taken off and let go), onResume() (its keys back), leaveGame() (back to its main menu: it calls
// closePause() first) and quitGame(). It binds Esc to pauseKey in its own maps.
//
// A game with a map rotation lets its host pick the map: its server sends the host's client (and no other) the maps it may pick
// ('MapChoices', then a 'MapChoice' for each), and the menu then has a Change map entry that lists them and asks the server
// for the one chosen (changeMap: commandToServer). The server decides who the host is and which maps are allowed; a game
// that sends no maps has no such entry.
//
// A game may add an Invite players entry: while $Pause::CanInvite is 1 the menu has it, and it calls the game's
// invitePlayers().
//
// The game lists its keys for the Controls panel once its client.cs has run this: addControls(%column, %title) starts a group
// of rows under a heading in column %column (0 or 1; the groups of a column stack), and addControl(%key, %action) adds a row
// to the last group. A game that lists none has no Controls entry. In a view too narrow for two columns the groups stack in
// one, and a list taller than the view scrolls. A game's resetCanvas() calls pauseResized().

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
new GuiControlProfile(PauseClearScrollProfile) { opaque = false; border = false; borderColor = "70 82 104 255"; fillColor = "16 20 28 255"; };
new GuiControlProfile(PauseGroupProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "150 160 178 255"; };
new GuiControlProfile(PauseKeyProfile) { fontType = "Arial"; fontSize = 18; fontColors[0] = "255 208 120 255"; };
new GuiControlProfile(PauseActionProfile) { fontType = "Arial"; fontSize = 18; fontColors[0] = "226 230 238 255"; };
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
         text = "Menu";
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
      new GuiButtonCtrl(InviteButton)
      {
         profile = PauseButtonProfile; position = "40 168"; extent = "240 56"; visible = false; enabled = true;
         text = "Invite players"; command = "invitePlayers();";
      };
      new GuiButtonCtrl(ControlsButton)
      {
         profile = PauseButtonProfile; position = "40 168"; extent = "240 56"; visible = false; enabled = true;
         text = "Controls"; command = "showControls();";
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
   // The game's keys: a key and what it does on each row, in groups (buildControls lays them out).
   new GuiControl(ControlsPanel)
   {
      profile = PausePanelProfile; position = "200 80"; extent = "400 440"; horizSizing = "center"; vertSizing = "center";
      visible = false;
      new GuiTextCtrl(ControlsTitle)
      {
         profile = PauseHeadProfile; position = "0 26"; extent = "400 40"; visible = true; enabled = true;
         text = "Controls";
      };
      new GuiScrollCtrl(ControlsScroll)
      {
         profile = PauseClearScrollProfile; position = "40 84"; extent = "320 260"; visible = true; enabled = true;
         hScrollBar = "alwaysOff"; vScrollBar = "dynamic";
         new GuiControl(ControlsList) { position = "0 0"; extent = "320 260"; visible = true; };
      };
      new GuiButtonCtrl(ControlsBackButton)
      {
         profile = PauseButtonProfile; position = "80 364"; extent = "240 48"; visible = true; enabled = true;
         text = "Back"; command = "hideControls();";
      };
      new GuiTextCtrl(ControlsNote)
      {
         profile = PauseHintProfile; position = "0 424"; extent = "400 20"; visible = true; enabled = true;
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
   hideControls();
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

// The menu with or without its Change map, Invite players and Controls entries: the buttons below them move down, and the panel grows to
// hold them.
function layoutPause()
{
   %y = 168;
   %host = $Client::MapCount > 0;
   ChangeMapButton.setVisible(%host);
   if (%host)
   {
      ChangeMapButton.resize(40, %y, 240, 56);
      %y += 76;
   }
   %invite = $Pause::CanInvite;
   InviteButton.setVisible(%invite);
   if (%invite)
   {
      InviteButton.resize(40, %y, 240, 56);
      %y += 76;
   }
   %controls = $Pause::GroupCount > 0;
   ControlsButton.setVisible(%controls);
   if (%controls)
   {
      ControlsButton.resize(40, %y, 240, 56);
      %y += 76;
   }
   LeaveButton.resize(40, %y, 240, 56);
   PauseQuitButton.resize(40, %y + 76, 240, 56);
   PauseNote.resize(0, %y + 148, 320, 20);
   %h = %y + 188;
   %size = PauseGui.getExtent();
   PausePanel.resize(mFloor((getWord(%size, 0) - 320) / 2), mFloor((getWord(%size, 1) - %h) / 2), 320, %h);
}

// ---- the game's keys

$Pause::GroupCount = 0;
// Each column of the list: the key's part, then what it does; the columns this far apart, the rows this far.
$Pause::KeyWidth = 140;
$Pause::ColumnWidth = 330;
$Pause::ColumnGap = 32;
$Pause::RowHeight = 24;

function addControls(%column, %title)
{
   %g = $Pause::GroupCount;
   $Pause::GroupCount++;
   $Pause::GroupColumn[%g] = %column;
   $Pause::GroupTitle[%g] = %title;
   $Pause::GroupRows[%g] = 0;
   $Pause::ControlsBuilt = 0;
}

function addControl(%key, %action)
{
   %g = $Pause::GroupCount - 1;
   if (%g < 0)
      return;
   %r = $Pause::GroupRows[%g];
   $Pause::GroupRows[%g] = %r + 1;
   $Pause::Key[%g, %r] = %key;
   $Pause::Action[%g, %r] = %action;
   $Pause::ControlsBuilt = 0;
}

// The rows of every group, each group under its heading in its column (in one column when %stacked); the list's size goes
// in $Pause::ControlsWidth and $Pause::ControlsHeight.
function buildControls(%stacked)
{
   ControlsList.clear();
   %columns = 1;
   %height = 0;
   %bottom[0] = 0;
   %bottom[1] = 0;
   %actionWidth = $Pause::ColumnWidth - $Pause::KeyWidth;
   for (%g = 0; %g < $Pause::GroupCount; %g++)
   {
      %c = ($Pause::GroupColumn[%g] && !%stacked) ? 1 : 0;
      if (%c + 1 > %columns)
         %columns = %c + 1;
      %x = %c * ($Pause::ColumnWidth + $Pause::ColumnGap);
      %y = %bottom[%c];
      if (%y > 0)
         %y += 16;
      if ($Pause::GroupTitle[%g] !$= "")
      {
         ControlsList.add(new GuiTextCtrl()
         {
            profile = PauseGroupProfile; position = %x SPC %y; extent = $Pause::ColumnWidth SPC 20; visible = true;
            enabled = true; text = $Pause::GroupTitle[%g];
         });
         %y += 26;
      }
      for (%r = 0; %r < $Pause::GroupRows[%g]; %r++)
      {
         ControlsList.add(new GuiTextCtrl()
         {
            profile = PauseKeyProfile; position = %x SPC %y; extent = $Pause::KeyWidth SPC 22; visible = true;
            enabled = true; text = $Pause::Key[%g, %r];
         });
         ControlsList.add(new GuiTextCtrl()
         {
            profile = PauseActionProfile; position = %x + $Pause::KeyWidth SPC %y; extent = %actionWidth SPC 22;
            visible = true; enabled = true; text = $Pause::Action[%g, %r];
         });
         %y += $Pause::RowHeight;
      }
      %bottom[%c] = %y;
      if (%y > %height)
         %height = %y;
   }
   $Pause::ControlsWidth = %columns * $Pause::ColumnWidth + (%columns - 1) * $Pause::ColumnGap;
   $Pause::ControlsHeight = %height;
   $Pause::ControlsStacked = %stacked;
   $Pause::ControlsBuilt = 1;
}

// Whether any group asks for the second column.
function controlsHaveTwoColumns()
{
   for (%g = 0; %g < $Pause::GroupCount; %g++)
      if ($Pause::GroupColumn[%g])
         return true;
   return false;
}

// The panel as large as its list, in the middle of the view: the groups in one column when two do not fit across it, and
// the list scrolled when it does not fit down it.
function layoutControls()
{
   %size = PauseGui.getExtent();
   %viewW = getWord(%size, 0);
   %viewH = getWord(%size, 1);
   %twoWidth = 2 * $Pause::ColumnWidth + $Pause::ColumnGap + 80;
   %stacked = controlsHaveTwoColumns() && %twoWidth > %viewW - 16;
   if (!$Pause::ControlsBuilt || %stacked != $Pause::ControlsStacked)
      buildControls(%stacked);
   %list = $Pause::ControlsHeight;
   // The title, the space under the list, the Back button and the note: what the panel holds besides the list.
   %room = %viewH - 16 - 84 - 116;
   %scroll = 0;
   if (%list > %room)
   {
      %list = getMax(%room, $Pause::RowHeight * 3);
      %scroll = 20;
   }
   %w = $Pause::ControlsWidth + 80 + %scroll;
   ControlsTitle.resize(0, 26, %w, 40);
   ControlsScroll.resize(40, 84, $Pause::ControlsWidth + %scroll, %list);
   ControlsList.resize(0, 0, $Pause::ControlsWidth, $Pause::ControlsHeight);
   ControlsScroll.scrollToTop();
   %y = 84 + %list + 20;
   ControlsBackButton.resize(mFloor((%w - 240) / 2), %y, 240, 48);
   ControlsNote.resize(0, %y + 60, %w, 20);
   ControlsNote.setText(%scroll ? "Mouse wheel: more keys    Esc: back to the game" : "Esc: back to the game");
   %h = %y + 96;
   ControlsPanel.resize(mFloor((%viewW - %w) / 2), mFloor((%viewH - %h) / 2), %w, %h);
}

// The window changed size: the menu laid out again for it.
function pauseResized()
{
   if (!$Client::Paused)
      return;
   layoutPause();
   if (ControlsPanel.isVisible())
      layoutControls();
}

function showControls()
{
   if (!$Client::Paused || $Pause::GroupCount < 1)
      return;
   layoutControls();
   PausePanel.setVisible(false);
   MapPanel.setVisible(false);
   ControlsPanel.setVisible(true);
}

function hideControls()
{
   ControlsPanel.setVisible(false);
   PausePanel.setVisible(true);
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
