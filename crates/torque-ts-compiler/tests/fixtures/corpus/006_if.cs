function my006f(%a, %b)
{
   if (%a)
      %x = 1;
   if (%a == 1)
      %x = 2;
   else
      %x = 3;
   if (%a $= "s")
   {
      %x = 4;
      %y = 5;
   }
   else if (%b + 1)
   {
      %x = 6;
   }
   else
   {
   }
   if ("s")
      %x = 7;
   if (1.5)
      %x = 8;
   if (%a.f)
      %x = 9;
   if (!%a)
      %x = 10;
   if (%a && %b)
   {
      if (%b)
         %x = 11;
   }
   return %x;
}
if ($mytop)
   $myy = 1;
else
   $myy = 2;
