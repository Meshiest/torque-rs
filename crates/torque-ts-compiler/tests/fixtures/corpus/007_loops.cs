function my007f(%n)
{
   while (%i < %n)
      %i++;
   for (%i = 0; %i < %n; %i++)
   {
      if (%i == 2)
         continue;
      if (%i == 5)
         break;
      %s = %s @ %i;
   }
   for (%j = 0; %j < 2; %j++)
      for (%k = 0; %k < 2; %k++)
         %t++;
   while (%n)
   {
      %n--;
      break;
   }
   while (1.5)
      break;
   return %s;
}
for ($myi = 0; $myi < 3; $myi++)
   $mys = $mys @ $myi;
while ($myi > 0)
   $myi--;
