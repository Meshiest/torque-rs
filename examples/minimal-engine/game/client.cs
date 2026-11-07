// The client: connectTo(%address) and disconnect(), and a canvas with a status line and a box for each ghost, seen from above.
// showGhosts() takes the status and the ghosts' positions (x y x y ...) and moves the boxes.

function connectTo(%address)
{
   %conn = new GameConnection(ServerConnection);
   %conn.connect(%address);
}

function disconnect()
{
   if (isObject(ServerConnection))
      ServerConnection.delete();
}

new GuiControlProfile(BackProfile) { opaque = true; fillColor = "24 28 36 255"; };
new GuiControlProfile(TextProfile) { fontType = "Arial"; fontSize = 16; fontColors[0] = "230 230 230 255"; };
new GuiControlProfile(BoxProfile) { opaque = true; fillColor = "240 180 60 255"; };

function buildGui()
{
   createCanvas("torque-example-engine");
   new GuiControl(ExampleGui)
   {
      profile = BackProfile; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
      new GuiTextCtrl(ExampleStatus) { profile = TextProfile; position = "8 8"; extent = "780 20"; text = "connecting"; };
   };
   for (%i = 0; %i < $Example::Boxes; %i++)
      ExampleGui.add(new GuiControl("ExampleBox" @ %i) { profile = BoxProfile; extent = "6 6"; visible = false; });
   Canvas.setContent(ExampleGui);
}

function showGhosts(%status, %positions)
{
   ExampleStatus.setText(%status);
   %canvas = Canvas.getExtent();
   for (%i = 0; %i < $Example::Boxes; %i++)
   {
      %box = "ExampleBox" @ %i;
      %x = getWord(%positions, %i * 2);
      %y = getWord(%positions, %i * 2 + 1);
      if (%y $= "")
         %box.setVisible(false);
      else
      {
         %box.resize(getWord(%canvas, 0) / 2 + %x * 4 - 3, getWord(%canvas, 1) / 2 - %y * 4 - 3, 6, 6);
         %box.setVisible(true);
      }
   }
}

buildGui();
