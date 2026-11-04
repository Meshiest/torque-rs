function my019f(%a, %b)
{
   %c = %a++;
   %d = -1;
   %e = %a = %b;
   %f = (%a = 2) + 1;
   %g = %a + %b = 3;
   %h = myfoo(%a ? %b : 1);
   %i = %a.f.g = 1;
   %j = break;
   return;
}
