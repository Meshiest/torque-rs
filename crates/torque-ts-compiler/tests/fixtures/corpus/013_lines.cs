function my013f(%a)
{
   %x = 1;

   // comment
   %y = 2; %z = 3;
   /* multi
      line */ %w = 4;
   mycall(1,
      2,
      3);
   %v = %a
      + 1
      ;
   if (%a)
   {
      %u = 5;
   }
   while (%a)
      %a--;
   return
      %x;
}
