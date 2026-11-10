// The prop catalog's rules, which run on the server only. A player's menu asks for a prop of a kind, with a label if it likes;
// everything it sends is checked here, because a modified client can send anything. The props are the map's (the level demo's
// props.rs names the first datablock of each kind in $Level::PropData["chair"] and so on) and spawn through the props plugin.
//
// What a player's menu sends:  hello  (show me the catalog)  |  spawn|kind|label  |  clear  (take back my props)
// Every one of them counts against the player's allowance of requests, so a menu that sends nothing but "hello" is held to it too.
// The game's own key for the menu (the level demo binds Q) reaches the server as the command Catalog, which answers a menu that
// said hello with "toggle": the server's script only reacts to a key the game chose.
// What the server sends back:  catalog|kind|name|kind|name|...   what can be spawned (to a menu that said hello)
//                              recent|player|kind|label|player|kind|label|...   the last spawns, oldest first (to every menu)
//                              toggle   open or close the menu (the player pressed the game's key for it)
//                              reply|ok|text  or  reply|refused|reason   to the player who asked
// The bar is the only separator: a message never holds a control character (the engine writes one as \u0009 and the like before a
// script sees it), and no name, label or reason holds a bar.

$Catalog::Kinds = "chair side_chair armchair sofa bench desk table cabinet copier fridge cooler bin lamp plant boxes monitor keyboard laptop mug magazines coffee_machine";
$Catalog::Names = "Chair|Side chair|Armchair|Sofa|Bench|Desk|Table|Cabinet|Copier|Fridge|Cooler|Bin|Lamp|Plant|Boxes|Monitor|Keyboard|Laptop|Mug|Magazines|Coffee maker";
// Props one player may have out at once.
$Catalog::MaxPerPlayer = 4;
// The longest label, and the characters a label may use.
$Catalog::MaxLabel = 32;
$Catalog::LabelChars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .,!?'-";
// The longest player name the recent list shows, and the characters it may use.
$Catalog::MaxName = 16;
$Catalog::NameChars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 _-.";
// Each player may ask this many times in a window of this many milliseconds; more is refused without being looked at.
$Catalog::PerWindow = 3;
$Catalog::WindowMs = 5000;
// How many recent spawns the server keeps.
$Catalog::MaxRecent = 10;

$Catalog::RecentCount = 0;
$Catalog::Spawned = 0;
$Catalog::Refused = 0;
$Catalog::Slowed = 0;

// The kinds this map has a datablock for, with their display names: "catalog|kind|Name|kind|Name...".
function catalogList()
{
   %names = strreplace($Catalog::Names, "|", "\t");
   %text = "catalog";
   for (%i = 0; %i < getWordCount($Catalog::Kinds); %i++)
   {
      %kind = getWord($Catalog::Kinds, %i);
      if (isObject($Level::PropData[%kind]))
         %text = %text @ "|" @ %kind @ "|" @ getField(%names, %i);
   }
   return %text;
}

function catalogKindName(%kind)
{
   %names = strreplace($Catalog::Names, "|", "\t");
   for (%i = 0; %i < getWordCount($Catalog::Kinds); %i++)
      if (getWord($Catalog::Kinds, %i) $= %kind)
         return getField(%names, %i);
   return %kind;
}

// What a player is called in the list. The name comes from the player's connect request and nothing verifies it (anyone may use any
// name, another player's included), so it is cut to $Catalog::MaxName characters and used only if it has nothing but the characters
// of $Catalog::NameChars; else the list says "Player".
function catalogPlayerName(%client)
{
   %name = trim(getSubStr(%client.name, 0, $Catalog::MaxName));
   if (%name !$= "" && stripChars(%name, $Catalog::NameChars) $= "")
      return %name;
   return "Player";
}

// Why `%label` cannot go on a prop, or "" when it can (no label is fine).
function catalogLabelProblem(%label)
{
   if (strlen(%label) > $Catalog::MaxLabel)
      return "That label is too long. Use" SPC $Catalog::MaxLabel SPC "characters or fewer.";
   if (stripChars(%label, $Catalog::LabelChars) !$= "")
      return "Labels can use letters, numbers, spaces and . , ! ? ' - only.";
   return "";
}

// Counts a request against `%client`'s allowance: true when they have used it up. Only the recent times are kept.
function catalogRateLimited(%client)
{
   %now = getSimTime();
   %times = %client.catalogTimes;
   %recent = "";
   %used = 0;
   for (%i = 0; %i < getWordCount(%times); %i++)
   {
      %at = getWord(%times, %i);
      if (%now - %at < $Catalog::WindowMs)
      {
         %recent = %recent SPC %at;
         %used++;
      }
   }
   if (%used >= $Catalog::PerWindow)
   {
      %client.catalogTimes = trim(%recent);
      return true;
   }
   %client.catalogTimes = trim(%recent SPC %now);
   return false;
}

function catalogSendRecent(%client)
{
   if (!isObject(%client))
      return;
   %text = "recent";
   for (%i = 0; %i < $Catalog::RecentCount; %i++)
      %text = %text @ "|" @ $Catalog::RecentName[%i] @ "|" @ $Catalog::RecentKind[%i] @ "|" @ $Catalog::RecentLabel[%i];
   clientScriptSend(%client, %text);
}

function catalogSendRecentToAll()
{
   for (%i = 0; %i < ClientGroup.getCount(); %i++)
   {
      %client = ClientGroup.getObject(%i);
      if (%client.catalogHere)
         catalogSendRecent(%client);
   }
}

function catalogReply(%client, %ok, %text)
{
   clientScriptSend(%client, "reply|" @ (%ok ? "ok" : "refused") @ "|" @ %text);
}

function catalogRefuse(%client, %why)
{
   $Catalog::Refused++;
   catalogReply(%client, false, %why);
}

// Remembers a spawn in the recent list; the oldest goes when it is full.
function catalogRemember(%player, %kind, %label)
{
   if ($Catalog::RecentCount >= $Catalog::MaxRecent)
   {
      for (%i = 1; %i < $Catalog::RecentCount; %i++)
      {
         $Catalog::RecentName[%i - 1] = $Catalog::RecentName[%i];
         $Catalog::RecentKind[%i - 1] = $Catalog::RecentKind[%i];
         $Catalog::RecentLabel[%i - 1] = $Catalog::RecentLabel[%i];
      }
      $Catalog::RecentCount--;
   }
   $Catalog::RecentName[$Catalog::RecentCount] = %player;
   $Catalog::RecentKind[$Catalog::RecentCount] = %kind;
   $Catalog::RecentLabel[$Catalog::RecentCount] = %label;
   $Catalog::RecentCount++;
}

// The player's request for a prop of `%kind` labeled `%label`: all the rules, then the spawn.
function catalogSpawn(%client, %kind, %label)
{
   // A kind is a word of lower-case letters and underscores that this map has a datablock for.
   %data = "";
   if (%kind !$= "" && stripChars(%kind, "abcdefghijklmnopqrstuvwxyz_") $= "")
      %data = $Level::PropData[%kind];
   if (!isObject(%data))
      return catalogRefuse(%client, "There is no such prop.");
   %why = catalogLabelProblem(%label);
   if (%why !$= "")
      return catalogRefuse(%client, %why);
   %player = %client.getControlObject();
   if (!isObject(%player) || %player.getState() $= "Dead")
      return catalogRefuse(%client, "You need to be standing in the office to put a prop down.");
   if (propCount(%client) >= $Catalog::MaxPerPlayer)
      return catalogRefuse(%client, "You already have" SPC $Catalog::MaxPerPlayer SPC "props out. Take them back first.");
   // In front of the player, on the floor they stand on: its middle half its height up, a little beyond its own size.
   %eye = %player.getEyePoint();
   %look = %player.getEyeVector();
   %flat = VectorNormalize(getWord(%look, 0) SPC getWord(%look, 1) SPC 0);
   %size = %data.size;
   %reach = 1 + getMax(getWord(%size, 0), getWord(%size, 1)) / 2;
   %feet = %player.getPosition();
   %at = (getWord(%eye, 0) + getWord(%flat, 0) * %reach) SPC (getWord(%eye, 1) + getWord(%flat, 1) * %reach)
      SPC (getWord(%feet, 2) + getWord(%size, 2) / 2 + 0.03);
   %why = propCheckPlacement(%data, %at, %eye, %player);
   if (%why !$= "")
      return catalogRefuse(%client, "There is no room for that in front of you:" SPC %why);
   %prop = propSpawn(%data, %at, %client);
   if (!%prop)
      return catalogRefuse(%client, "The server could not put that down.");
   // The label stays on the prop for as long as it is in the world.
   %prop.catalogKind = %kind;
   %prop.catalogLabel = %label;
   %prop.catalogOwner = %client.getId();
   catalogPrune(%client);
   %client.catalogProps = trim(%client.catalogProps SPC %prop);
   $Catalog::Spawned++;
   catalogRemember(catalogPlayerName(%client), catalogKindName(%kind), %label);
   catalogReply(%client, true, catalogKindName(%kind) SPC "put down in front of you.");
   catalogSendRecentToAll();
}

// Forgets the ids of `%client`'s props that are gone, or are no longer theirs (an id may be given to another object later).
function catalogPrune(%client)
{
   %kept = "";
   for (%i = 0; %i < getWordCount(%client.catalogProps); %i++)
   {
      %prop = getWord(%client.catalogProps, %i);
      if (isObject(%prop) && %prop.catalogOwner == %client.getId())
         %kept = %kept SPC %prop;
   }
   %client.catalogProps = trim(%kept);
}

// Takes back the props the player has out, and only theirs.
function catalogClear(%client)
{
   %taken = 0;
   catalogPrune(%client);
   for (%i = 0; %i < getWordCount(%client.catalogProps); %i++)
   {
      %prop = getWord(%client.catalogProps, %i);
      if (propDelete(%prop))
         %taken++;
   }
   %client.catalogProps = "";
   catalogReply(%client, true, "Took back" SPC %taken SPC "of your props.");
}

// A message from a player's menu. `%text` is whatever the player's machine sent.
function clientScriptOnMessage(%client, %text)
{
   if (catalogRateLimited(%client))
   {
      $Catalog::Slowed++;
      catalogReply(%client, false, "Slow down. Wait a few seconds and try again.");
      return;
   }
   if (%text $= "hello")
   {
      %client.catalogHere = true;
      clientScriptSend(%client, catalogList());
      catalogSendRecent(%client);
      return;
   }
   // The bars become tabs, which getField splits on.
   %fields = strreplace(%text, "|", "\t");
   %what = getField(%fields, 0);
   if (%what $= "clear")
   {
      catalogClear(%client);
      return;
   }
   if (%what !$= "spawn")
   {
      catalogRefuse(%client, "The server did not understand that.");
      return;
   }
   // A bar inside a label makes more than three fields: the label is the problem, as with any character the list leaves out.
   if (getFieldCount(%fields) > 3)
      return catalogRefuse(%client, "Labels can use letters, numbers, spaces and . , ! ? ' - only.");
   catalogSpawn(%client, getField(%fields, 1), trim(getField(%fields, 2)));
}

// The player pressed the game's key for the menu (the level demo's Q): a menu that is up is told to open or close.
function serverCmdCatalog(%client)
{
   if (%client.catalogHere)
      clientScriptSend(%client, "toggle");
}
