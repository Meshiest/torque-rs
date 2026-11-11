// The test game's client: connectTo(%address), and the commands the server sends logged as the script sees them.

function connectTo(%address)
{
   %conn = new GameConnection(ServerConnection);
   %conn.connect(%address);
}

function GameConnection::onConnectionAccepted(%this)
{
   $Accepted = 1;
}

function clientLine(%text)
{
   $ClientLog = $ClientLog @ %text @ "\n";
}

function clientCmdWelcome(%msg)
{
   clientLine("Welcome(" @ detag(%msg) @ ")");
}

function clientCmdReply(%a, %d)
{
   clientLine("Reply(" @ %a @ "|" @ %d @ ")");
}

function clientCmdNotice(%msg, %a, %b)
{
   clientLine("Notice(" @ detag(%msg) @ ")");
}
