// The test game's server: counts the commands a client's calls reach, apart from the plugin's native ones.

function startIt()
{
   new SimGroup(ClientGroup);
}

function GameConnection::onConnect(%client)
{
   ClientGroup.add(%client);
   $Client = %client;
   $Connected++;
}

// A command no plugin declared: TGE's behavior, no limits.
function serverCmdLegacy(%client, %a)
{
   $Legacy++;
}

// A command whose function is here and whose limits a plugin declared.
function serverCmdScripted(%client, %n)
{
   $Scripted++;
}
