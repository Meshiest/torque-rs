function my002f(%a, %b, %c)
{
   %r = 1 + 2 * 3 - 4 / 5 % 6;
   %r = %a + %b - %c * %a / %b;
   %r = %a % %b & %c | %a ^ %b;
   %r = %a << 2 >> %b;
   %r = %a < %b && %b > %c || %a <= %c && %b >= %a;
   %r = %a == %b != %c;
   %r = %a $= %b;
   %r = %a !$= "x";
   %r = %a @ %b SPC %c TAB %a NL %b;
   %r = -%a;
   %r = !%a;
   %r = ~%a;
   %r = !%a == %b;
   %r = -%a.f;
   %r = %a ? %b : %c;
   %r = %a ? %b : %c ? 1 : 2;
   %r = %a ? 1.5 : "s";
   %r = (%a + %b) * %c;
   %r = %a @ %b + %c;
   %r = %a + %b @ %c;
   %r = %a $= %b @ %c;
   %r = !%a @ %b;
   %r = !"s";
   %r = !1.5;
   %r = !%a.f;
   %r = - - %a;
   %r = %a - -1;
   return %r;
}
