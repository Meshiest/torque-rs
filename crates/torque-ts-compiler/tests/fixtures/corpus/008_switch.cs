function my008f(%a)
{
   switch (%a)
   {
      case 1:
         %x = 1;
      case 2 or 3:
         %x = 2;
      case 4 or 5 or 6:
         %x = 3;
         %y = 4;
      default:
         %x = 5;
   }
   switch$ (%a)
   {
      case "a":
         %x = "a";
      case "b" or "c":
         %x = "b";
   }
   for (%i = 0; %i < 3; %i++)
   {
      switch (%i)
      {
         case 1:
            break;
         case 2:
            continue;
      }
   }
   switch (%a + 1)
   {
      case 7:
   }
   return %x;
}
