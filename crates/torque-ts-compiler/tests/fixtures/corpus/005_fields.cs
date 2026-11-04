function my005f(%o)
{
   %a = %o.f;
   %b = %o.f[1];
   %o.f[1, 2] = 3;
   %o.f = %o.g;
   $myx.y.z = 1;
   %c = %o.f.g.h;
   %d = %o.f[%a].g[%b];
   %o.f = "s";
   %e = %o.f + 1;
   %g = %o.f @ "s";
   return %o.f;
}
