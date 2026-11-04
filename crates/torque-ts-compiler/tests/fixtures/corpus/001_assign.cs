$mya = 1;
$myb = 1.5;
$myc = "s";
$myd = 'tagd';
%myl = 2;
$mye[1] = 3;
$myf[1, 2] = "x";
$myg[$mya] = $myb;
$myh = $mya = $myb;
$myi = $mya;
$myj[1] = $mye[1];
$myk = %myl;
function my001f(%a)
{
   %b = %a;
   %c[%a, %b, 3] = %a[1];
   return %c;
}
