// The examples' chat: a box of the last lines in the view, and a line to type into. T opens it, Enter sends what was typed
// and Esc closes it. While it is open the mouse is free (the cursor on, as over the pause menu: a browser lets go of the
// pointer lock) and the game's keys are off its figure, so typing moves nothing. $Chat::Open is 1 while it is.
//
// A game's client.cs execs this after common/pause.cs and gives the hooks: pauseAllowed() (the pause menu's: whether the
// game is in its view now), onChatOpen() (its keys taken off the figure and the held ones let go: a key that comes up while
// the chat is open is not seen) and onChatClose() (the keys of where the figure is back). It calls makeChat(%view, %x, %y,
// %vertSizing) to put the box into its view, and binds T to chatKey in its own maps. The pause menu does not open over the
// chat (Esc closes the chat first).
//
// The lines come from the server ('ChatMessage': a player's line, or a line from the game itself, in its own color), as
// data: the client only shows them. $Chat::Last is the newest line and $Chat::Count counts them. The server's side (the
// limits on what a player may send) is common/server.cs's.

new GuiControlProfile(ChatBandProfile) { opaque = true; fillColor = "10 12 18 150"; };
new GuiControlProfile(ChatLineProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "236 240 248 255"; };
new GuiControlProfile(ChatInfoProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "255 208 120 255"; };
new GuiControlProfile(ChatEditProfile)
{
   opaque = true;
   border = true;
   fillColor = "16 20 28 230";
   fillColorHL = "90 110 150 255";
   borderColor = "255 190 90 255";
   fontType = "Arial";
   fontSize = 18;
   fontColors[0] = "240 242 248 255";
   fontColorHL = "255 255 255 255";
   cursorColor = "255 208 120 255";
   canKeyFocus = true;
   autoSizeHeight = false;
};

// The rows the box shows, and the longest line the edit takes (the server cuts longer ones too).
$Chat::Rows = 6;
$Chat::MaxLength = 120;
$Chat::Open = 0;
$Chat::Count = 0;

// The lines the box shows, the newest last: each line's tag is 1 for a line from the game, 0 for a player's.
new MessageVector(ChatLines);

// Puts the chat box into %view with its top left at %x %y, the line to type into under it; %vertSizing ("top" for a box
// at the bottom of the view) keeps it where it is as the view changes size. Each row is two texts (a player's line, a line
// from the game), one of them empty.
function makeChat(%view, %x, %y, %vertSizing)
{
   %h = 12 + $Chat::Rows * 20;
   %view.add(new GuiControl(ChatBox)
   {
      profile = ChatBandProfile; position = %x SPC %y; extent = "460" SPC %h; vertSizing = %vertSizing;
   });
   %view.add(new GuiTextEditCtrl(ChatEdit)
   {
      profile = ChatEditProfile; position = %x SPC %y + %h + 6; extent = "460 30"; vertSizing = %vertSizing;
      visible = false; enabled = true; maxLength = $Chat::MaxLength; text = "";
      altCommand = "sendChat();"; escapeCommand = "closeChat();";
   });
   for (%i = 0; %i < $Chat::Rows; %i++)
   {
      %row = 6 + %i * 20;
      ChatBox.add(new GuiTextCtrl("ChatRow" @ %i)
      {
         profile = ChatLineProfile; position = "10" SPC %row; extent = "440 20"; visible = true; enabled = true; text = "";
      });
      ChatBox.add(new GuiTextCtrl("InfoRow" @ %i)
      {
         profile = ChatInfoProfile; position = "10" SPC %row; extent = "440 20"; visible = true; enabled = true; text = "";
      });
   }
}

// A line in the box: %info 1 for a line from the game, 0 for a player's.
function chatLine(%info, %text)
{
   ChatLines.pushBackLine(%text, %info ? 1 : 0);
   $Chat::Count++;
   $Chat::Last = %text;
   while (ChatLines.getNumLines() > $Chat::Rows)
      ChatLines.popFrontLine();
   showChat();
}

function showChat()
{
   %count = ChatLines.getNumLines();
   for (%i = 0; %i < $Chat::Rows; %i++)
   {
      %text = %i < %count ? ChatLines.getLineText(%i) : "";
      %info = %i < %count ? ChatLines.getLineTag(%i) : 0;
      %chat = "ChatRow" @ %i;
      %line = "InfoRow" @ %i;
      %chat.setText(%info ? "" : %text);
      %line.setText(%info ? %text : "");
   }
}

// Empties the box (the game was left).
function clearChat()
{
   closeChat();
   ChatLines.clear();
   showChat();
}

function openChat()
{
   if ($Chat::Open || $Client::Paused || !pauseAllowed())
      return;
   $Chat::Open = 1;
   onChatOpen();
   Canvas.cursorOn();
   ChatMap.push();
   ChatEdit.setValue("");
   ChatEdit.setVisible(true);
   ChatEdit.makeFirstResponder(true);
   // The key that opened the box types its letter into it a moment later: take it out again.
   schedule(1, 0, "clearChatEdit");
}

function clearChatEdit()
{
   ChatEdit.setValue("");
}

function closeChat()
{
   if (!$Chat::Open)
      return;
   $Chat::Open = 0;
   ChatEdit.setValue("");
   ChatEdit.setVisible(false);
   ChatEdit.makeFirstResponder(false);
   ChatMap.pop();
   Canvas.cursorOff();
   onChatClose();
}

function sendChat()
{
   %text = trim(ChatEdit.getValue());
   closeChat();
   if (%text !$= "")
      say(%text);
}

function say(%text)
{
   if (isObject(ServerConnection))
      commandToServer('Say', %text);
}

function chatKey(%v) { if (%v) openChat(); }

// While the chat is open, Esc closes it even when the line to type into is not the one taking the keys (a click elsewhere).
new ActionMap(ChatMap);
function chatEscapeKey(%v) { if (%v) closeChat(); }
ChatMap.bind(keyboard, escape, chatEscapeKey);

// %kind is "chat" (a player's line: %name and %text) or "info" (a line from the game: %name is the line).
function clientCmdChatMessage(%kind, %name, %text)
{
   if (%kind $= "chat")
      chatLine(0, %name @ ": " @ %text);
   else
      chatLine(1, %name);
}
