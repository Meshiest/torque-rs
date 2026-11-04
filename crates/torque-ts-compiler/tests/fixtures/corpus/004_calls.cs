mynone();
mynone(1, "a", $myx, 1.5);
function my004f(%o)
{
   myfoo();
   myfoo(1, "a", %o, 2.5);
   MyNs::mybar(%o);
   Parent::mybaz(%o, 1);
   %o.mym(1);
   %o.f.mym2();
   ("myo").mym3();
   %x = myfoo(myfoo(1), 2) + 1;
   %y = %o.mym() * 2;
   if (myfoo())
      myfoo();
   return myfoo() @ "x";
}
