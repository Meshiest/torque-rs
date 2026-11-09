// The test game's server: every command a client sends is logged as the script sees it.

function startIt()
{
   new SimGroup(ClientGroup);
}

function line(%text)
{
   $Log = $Log @ %text @ "\n";
}

function who(%client)
{
   return %client == $Client ? "C" : "other" @ %client;
}

function GameConnection::onConnect(%client)
{
   ClientGroup.add(%client);
   $Client = %client;
   $Connected++;
   commandToClient(%client, 'Welcome', 'Hello %1', "there");
}

function serverCmdEcho(%client, %a, %b, %c, %d)
{
   line("Echo(" @ who(%client) @ "|" @ %a @ "|" @ %b @ "|" @ %c @ "|" @ %d @ ")");
   commandToClient(%client, 'Reply', %a, %d);
}

function serverCmdTagged(%client, %a, %b)
{
   line("Tagged(" @ who(%client) @ "|" @ detag(%a) @ "|" @ %b @ ")");
   commandToClient(%client, 'Notice', 'Seen %1 of %2', detag(%a), %b);
}

function serverCmdLong(%client, %a)
{
   line("Long(" @ strlen(%a) @ "|" @ getSubStr(%a, 250, 10) @ ")");
}

function serverCmdMany(%client, %a1, %a2, %a3, %a4, %a5, %a6, %a7, %a8, %a9, %a10, %a11, %a12, %a13, %a14, %a15, %a16, %a17, %a18, %a19)
{
   line("Many(" @ %a1 @ "|" @ %a10 @ "|" @ %a19 @ ")");
}

function serverCmdEmpty(%client, %a, %b, %c)
{
   line("Empty(" @ %a @ "|" @ %b @ "|" @ %c @ ")");
}

function serverCmdOrder(%client, %n)
{
   $Order = $Order @ %n @ ",";
}

function serverCmdEarly(%client)
{
   $Early++;
}

// A command whose limits a plugin declared (rate and permission).
function serverCmdFlood(%client, %n)
{
   $Flood++;
   $FloodSeq = $FloodSeq @ %n @ ",";
}

function serverCmdGated(%client)
{
   $Gated++;
}

// A command no plugin declared.
function serverCmdUnlisted(%client, %n)
{
   $Unlisted++;
}
