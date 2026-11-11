// What the example games share for playing with others: the screen that asks who may join a match hosted here, the list of
// the network's matches with a box for an address, and the join screen, which shows the server's mods coming in (the mods and
// their version, their files and size, a bar) before the game.
//
// A game's client.cs execs this once its canvas exists, and gives it first:
// - the menus' profiles: ClearProfile, MenuBackProfile, MenuPanelProfile, MenuTitleProfile, MenuHeadProfile, MenuModProfile
//   (right-justified: the mods' names end at the join screen's middle, their versions start beside them), MenuTextProfile,
//   MenuCenterProfile, MenuDimProfile, MenuDimCenterProfile, MenuNoteProfile, MenuTrackProfile, MenuBarProfile,
//   MenuListProfile, MenuScrollProfile, MenuEditProfile and MenuButtonProfile;
// - the hooks: disconnect() (ends the connection), showMainMenu() (its menu, the Back of these screens), joinScreenShown() (the
//   join screen came up: its game keys off), joinLeft(%why) (a join or a game was left: its keys let go, its menu shown and
//   %why on it when there is one), and saveName() if its menu has a field for the player's name;
// - $Game::Port, the UDP port number the list asks on; $Join::ModsLabel, what the join screen calls the server's mods ("The
//   server's mods" if unset); and $Join::NoBots = 1 to leave the list's Bots column out.
//
// joinServer(%address, %shown) joins a server through the join screen. The game's GameConnection callbacks pass a join on while
// $Join::State is set (joinAccepted, joinRejected(%why), joinTimedOut, joinDropped(%why)), and the game calls joinPlaying() when
// the server has put the player in the game. $Join::State says where a join stands: connecting, accepted, downloading, cached,
// joining, playing or failed; empty when the client is in no join. $Join::Seen lists every state the join went through, in
// order, and $Join::Mods and $Join::Versions the offer's mod display names and versions, tab separated. Hosting sets
// $Game::HostAsked (with $Game::HostScope "lan" or "public"), which the host program watches.

// ---- joining

// Joins the server at %address; the join screen names it %shown when given (a page names the server it came from, not the
// address its game socket stands for).
function joinServer(%address, %shown)
{
   disconnect();
   $Join::Address = %address;
   $Join::Shown = %shown $= "" ? %address : %shown;
   $Join::Mods = "";
   $Join::Versions = "";
   $Join::Files = 0;
   $Join::Bytes = 0;
   $Join::Received = 0;
   $Join::Seen = "";
   setJoinState("connecting");
   %conn = new GameConnection(ServerConnection);
   if ($Pref::Player::Name !$= "")
      %conn.setConnectArgs($Pref::Player::Name);
   %conn.connect(%address);
   showJoinScreen();
}

function joinAccepted()
{
   setJoinState("accepted");
}

function joinRejected(%why)
{
   joinFailed("The server turned you away: " @ %why);
}

function joinTimedOut()
{
   joinFailed("No server answered at " @ $Join::Shown @ ".");
}

// The connection ended: in the game, back to the menu with why; while joining, on the join screen.
function joinDropped(%why)
{
   if ($Join::State $= "playing")
      leaveJoin(%why);
   else
      joinFailed(%why);
}

// The server put the player in the game.
function joinPlaying()
{
   setJoinState("playing");
}

// The mods the server needs (their names and versions, each list tab separated: a name has no tab) and their files, from the offer it sent.
function readOffer()
{
   %list = getRequiredModList();
   $Join::Mods = "";
   $Join::Versions = "";
   $Join::Files = 0;
   for (%i = 0; %i < getWordCount(%list); %i++)
   {
      %id = getWord(%list, %i);
      %name = getRequiredModField(%id, "name");
      %version = getRequiredModField(%id, "version");
      $Join::Mods = $Join::Mods $= "" ? %name : $Join::Mods @ "	" @ %name;
      $Join::Versions = $Join::Versions $= "" ? %version : $Join::Versions @ "	" @ %version;
      $Join::Files += getRequiredModField(%id, "files");
   }
}

// The download's callbacks. A connection made without the join screen (a game played alone) has no join to show them on.
function onModDownloadStarted(%mods, %bytes)
{
   if ($Join::State $= "")
      return;
   $Download::Started = %mods SPC %bytes;
   readOffer();
   $Join::Bytes = %bytes;
   $Join::Received = 0;
   setJoinState(%mods > 0 ? "downloading" : "cached");
}

function onModDownloadProgress(%id, %received, %total, %index, %count)
{
   if ($Join::State $= "")
      return;
   $Join::Received = %received;
   $Join::Bytes = %total;
   updateJoinScreen();
}

function onModDownloadDone()
{
   if ($Join::State $= "")
      return;
   $Download::Done++;
   $Join::Received = $Join::Bytes;
   if ($Join::State !$= "cached")
      setJoinState("joining");
}

function onModDownloadFailed(%why)
{
   if ($Join::State $= "")
      return;
   $Download::Failed = %why;
   joinFailed("The download stopped: " @ %why);
}

function joinFailed(%why)
{
   $Join::Why = %why;
   setJoinState("failed");
}

// Out of the server, back to the game's menu (the join screen's Cancel and Back, and a game left), with %why on it if given.
function leaveJoin(%why)
{
   disconnect();
   $Join::State = "";
   joinLeft(%why);
}

// A size in bytes as a player reads it.
function sizeText(%bytes)
{
   if (%bytes < 1024)
      return %bytes @ " bytes";
   if (%bytes < 1048576)
      return mFloor(%bytes / 102.4) / 10 @ " KB";
   return mFloor(%bytes / 104857.6) / 10 @ " MB";
}

// $Join::Seen keeps every state this join went through, in order, for a check that looks only now and then.
function setJoinState(%state)
{
   if (%state !$= $Join::State || $Join::Seen $= "")
      $Join::Seen = trim($Join::Seen SPC %state);
   $Join::State = %state;
   updateJoinScreen();
}

// The join screen as the join stands now.
function updateJoinScreen()
{
   %state = $Join::State;
   %label = $Join::ModsLabel $= "" ? "The server's mods" : $Join::ModsLabel;
   JoinScreenTitle.setText("Joining " @ $Join::Shown);
   %offer = %state $= "downloading" || %state $= "cached" || %state $= "joining";
   JoinScreenMod.setText(%offer ? strreplace($Join::Mods, "	", ", ") : "");
   JoinScreenVersion.setText(%offer ? strreplace($Join::Versions, "	", ", ") : "");
   %files = $Join::Files == 1 ? "1 file" : $Join::Files @ " files";
   if (%state $= "cached")
      JoinScreenDetail.setText(%label @ ", already on this computer.");
   else if (%offer)
      JoinScreenDetail.setText(%label @ ": " @ %files @ ", " @ sizeText($Join::Bytes) @ " to download.");
   else
      JoinScreenDetail.setText("");
   JoinBarTrack.setVisible(%offer);
   %done = $Join::Bytes > 0 ? $Join::Received / $Join::Bytes : 1;
   JoinBar.setValue(%state $= "downloading" ? %done : 1);
   if (%state $= "connecting")
      %status = "Connecting...";
   else if (%state $= "accepted")
      %status = "Connected. Asking the server which mods it plays...";
   else if (%state $= "downloading")
      %status = "Downloading: " @ sizeText($Join::Received) @ " of " @ sizeText($Join::Bytes);
   else if (%state $= "cached")
      %status = "Nothing to download. Joining the match...";
   else if (%state $= "joining")
      %status = "Downloaded. Joining the match...";
   else if (%state $= "failed")
      %status = $Join::Why;
   else
      %status = "";
   JoinScreenStatus.setText(%status);
   JoinScreenBack.setText(%state $= "failed" ? "Back" : "Cancel");
}

function showJoinScreen()
{
   joinScreenShown();
   Canvas.setContent(JoinScreenGui);
   Canvas.cursorOn();
   updateJoinScreen();
}

function saveTypedName()
{
   if (isFunction("saveName"))
      saveName();
}

// ---- who may join a match hosted here

new GuiControl(HostGui)
{
   profile = MenuBackProfile; visible = true; enabled = true; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(HostPanel)
   {
      profile = ClearProfile; visible = true; enabled = true; position = "40 50"; extent = "720 500"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(HostTitle) { profile = MenuTitleProfile; visible = true; enabled = true; position = "20 20"; extent = "660 56"; text = "Host a match"; };
      new GuiTextCtrl(HostLine) { profile = MenuDimProfile; visible = true; enabled = true; position = "22 80"; extent = "660 20"; text = "Who can join the match you start?"; };
      new GuiButtonCtrl(HostLanButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "20 140"; extent = "280 56"; text = "LAN only"; command = "hostGame(\"lan\");";
      };
      new GuiTextCtrl(HostLanNote) { profile = MenuDimProfile; visible = true; enabled = true; position = "322 158"; extent = "380 20"; text = "Players on your own network can join."; };
      new GuiButtonCtrl(HostPublicButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "20 212"; extent = "280 56"; text = "Public"; command = "hostGame(\"public\");";
      };
      new GuiTextCtrl(HostPublicNote)
      {
         profile = MenuDimProfile; visible = true; enabled = true; position = "322 222"; extent = "380 20"; text = "Anyone who can reach this computer can join.";
      };
      new GuiTextCtrl(HostPublicNote2)
      {
         profile = MenuDimProfile; visible = true; enabled = true; position = "322 244"; extent = "380 20"; text = "Needs UDP 28000 forwarded to this computer.";
      };
      new GuiButtonCtrl(HostBackButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "20 284"; extent = "280 56"; text = "Back"; command = "showMainMenu();";
      };
      new GuiTextCtrl(HostStatus) { profile = MenuNoteProfile; visible = true; enabled = true; position = "20 450"; extent = "680 28"; text = ""; };
   };
};

function showHost()
{
   saveTypedName();
   HostStatus.setText("");
   HostPublicNote2.setText("Needs UDP" SPC ($Game::Port $= "" ? 28000 : $Game::Port) SPC "forwarded to this computer.");
   Canvas.setContent(HostGui);
   Canvas.cursorOn();
}

// %scope is "lan" or "public".
function hostGame(%scope)
{
   $Game::HostScope = %scope $= "public" ? "public" : "lan";
   saveTypedName();
   HostStatus.setText("Starting a match...");
   $Game::HostAsked = 1;
}

// ---- the network's matches

new GuiControl(JoinGui)
{
   profile = MenuBackProfile; visible = true; enabled = true; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(JoinPanel)
   {
      profile = ClearProfile; visible = true; enabled = true; position = "20 30"; extent = "760 540"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(JoinTitle) { profile = MenuTitleProfile; visible = true; enabled = true; position = "0 0"; extent = "700 56"; text = "Join a match"; };
      new GuiTextCtrl(JoinHeaderGame) { profile = MenuDimProfile; visible = true; enabled = true; position = "4 74"; extent = "200 20"; text = "Server"; };
      new GuiTextCtrl(JoinHeaderPlayers) { profile = MenuDimProfile; visible = true; enabled = true; position = "404 74"; extent = "90 20"; text = "Players"; };
      new GuiTextCtrl(JoinHeaderBots)
      {
         profile = MenuDimProfile; visible = !$Join::NoBots; enabled = true; position = "504 74"; extent = "90 20"; text = "Bots";
      };
      new GuiTextCtrl(JoinHeaderPing)
      {
         profile = MenuDimProfile; visible = true; enabled = true; position = ($Join::NoBots ? "504" : "604") SPC "74"; extent = "80 20"; text = "Ping";
      };
      new GuiScrollCtrl(ServerScroll)
      {
         profile = MenuScrollProfile; visible = true; enabled = true; position = "0 98"; extent = "760 252"; hScrollBar = "alwaysOff"; vScrollBar = "dynamic";
         new GuiTextListCtrl(ServerList)
         {
            profile = MenuListProfile; visible = true; enabled = true; position = "2 2"; extent = "740 20";
            columns = $Join::NoBots ? "0 400 500" : "0 400 500 600";
         };
      };
      new GuiTextCtrl(AddressLabel) { profile = MenuTextProfile; visible = true; enabled = true; position = "4 372"; extent = "76 24"; text = "Address"; };
      new GuiTextEditCtrl(AddressEdit)
      {
         profile = MenuEditProfile; visible = true; enabled = true; position = "82 368"; extent = "498 32"; maxLength = 260; text = "";
         placeholderText = "127.0.0.1:28000"; altCommand = "connectAddress();";
      };
      new GuiButtonCtrl(ConnectButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "590 364"; extent = "170 40"; text = "Connect"; command = "connectAddress();";
      };
      new GuiButtonCtrl(RefreshButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "0 440"; extent = "200 50"; text = "Refresh"; command = "refreshServers();";
      };
      new GuiButtonCtrl(JoinSelectedButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "280 440"; extent = "200 50"; text = "Join"; command = "joinSelected();";
      };
      new GuiButtonCtrl(BackButton)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "560 440"; extent = "200 50"; text = "Back"; command = "showMainMenu();";
      };
      new GuiTextCtrl(JoinStatus) { profile = MenuNoteProfile; visible = true; enabled = true; position = "0 506"; extent = "760 28"; text = ""; };
   };
};

function showJoin()
{
   saveTypedName();
   AddressEdit.setValue($Pref::Client::LastAddress);
   Canvas.setContent(JoinGui);
   Canvas.cursorOn();
   refreshServers();
}

function refreshServers()
{
   ServerList.clear();
   $Game::ServerRows = 0;
   JoinStatus.setText("Looking for matches...");
   queryLanServers($Game::Port, 0, "", "", 0, 100, 0, 2, 0, 100, 0);
}

function onServerQueryStatus(%status, %message, %value)
{
   if (%status !$= "done")
      return;
   fillServerList();
   JoinStatus.setText(getServerCount() == 0 ? "No matches found. Host one, or refresh." : "Pick a match and press Join.");
}

function fillServerList()
{
   ServerList.clear();
   for (%i = 0; %i < getServerCount(); %i++)
   {
      if (!setServerInfo(%i))
         continue;
      %row = $ServerInfo::Name TAB $ServerInfo::PlayerCount @ " / " @ $ServerInfo::MaxPlayers;
      if (!$Join::NoBots)
         %row = %row TAB $ServerInfo::BotCount;
      ServerList.addRow(%i, %row TAB $ServerInfo::Ping);
   }
   $Game::ServerRows = ServerList.rowCount();
   if ($Game::ServerRows > 0)
      ServerList.setSelectedRow(0);
}

function joinSelected()
{
   %row = ServerList.getSelectedId();
   if (%row $= "" || %row < 0 || !setServerInfo(%row))
   {
      JoinStatus.setText("Pick a match first.");
      return;
   }
   cancelJoinLookup();
   joinServer($ServerInfo::Address);
}

// Connects to the address or name typed in the box (with :port, or without: port 28000); a name is looked up first.
function connectAddress()
{
   saveTypedName();
   %typed = trim(AddressEdit.getValue());
   %address = checkJoinAddress(%typed);
   if (getSubStr(%address, 0, 1) $= "!")
   {
      JoinStatus.setText(getSubStr(%address, 1, 200));
      return;
   }
   $Pref::Client::LastAddress = %typed;
   if (getSubStr(%address, 0, 1) $= "?")
   {
      JoinStatus.setText("Looking up " @ getSubStr(%address, 1, 260) @ "...");
      schedule(50, 0, pollJoinLookup);
      return;
   }
   armPublicJoin(%address);
   joinServer(%address);
}

// Connects once the typed name's lookup ends; leaving the screen drops it.
function pollJoinLookup()
{
   if (Canvas.getContent() != JoinGui.getId())
   {
      cancelJoinLookup();
      return;
   }
   %address = joinLookupResult();
   if (%address $= "?")
      schedule(50, 0, pollJoinLookup);
   else if (getSubStr(%address, 0, 1) $= "!")
      JoinStatus.setText(getSubStr(%address, 1, 200));
   else if (%address !$= "")
      joinServer(%address);
}

// ---- the join screen: the server's mods coming in, then the game

new GuiControl(JoinScreenGui)
{
   profile = MenuBackProfile; visible = true; enabled = true; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
   new GuiControl(JoinScreenPanel)
   {
      profile = MenuPanelProfile; visible = true; enabled = true; position = "120 160"; extent = "560 280"; horizSizing = "center"; vertSizing = "center";
      new GuiTextCtrl(JoinScreenTitle) { profile = MenuHeadProfile; visible = true; enabled = true; position = "20 22"; extent = "520 38"; text = ""; };
      // The mods' names end at the middle (MenuModProfile is right-justified) and their versions, smaller, start beside them.
      new GuiTextCtrl(JoinScreenMod) { profile = MenuModProfile; visible = true; enabled = true; position = "20 76"; extent = "256 30"; text = ""; };
      new GuiTextCtrl(JoinScreenVersion) { profile = MenuDimProfile; visible = true; enabled = true; position = "286 84"; extent = "254 22"; text = ""; };
      new GuiTextCtrl(JoinScreenDetail) { profile = MenuDimCenterProfile; visible = true; enabled = true; position = "20 110"; extent = "520 22"; text = ""; };
      new GuiControl(JoinBarTrack)
      {
         profile = MenuTrackProfile; visible = true; enabled = true; position = "40 146"; extent = "480 24";
         new GuiProgressCtrl(JoinBar) { profile = MenuBarProfile; visible = true; enabled = true; position = "2 2"; extent = "476 20"; };
      };
      new GuiTextCtrl(JoinScreenStatus) { profile = MenuCenterProfile; visible = true; enabled = true; position = "20 182"; extent = "520 24"; text = ""; };
      new GuiButtonCtrl(JoinScreenBack)
      {
         profile = MenuButtonProfile; visible = true; enabled = true; position = "190 220"; extent = "180 44"; text = "Cancel"; command = "leaveJoin(\"\");";
      };
   };
};
