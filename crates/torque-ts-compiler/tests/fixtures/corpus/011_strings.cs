$mys1 = "a\tb\nc\d\"e\'f";
$mys2 = "\x41\x7a\c0\c1\c9\cr\cp\co";
$mys3 = "dup";
$mys4 = "dup";
$mys5 = "Dup";
$mys6 = 'dup';
$mys7 = 'abcdef';
$mys8 = 'abcdefg';
$mys9 = "";
$mysA = '';
$mysB = myidentvalue;
$mysC = myidentvalue + 1;
$mysD = "1.5" + 1;
$mysE = "abc" * 1;
$mysF = "true" + 0;
$mysG = "0.1" + 0;
$mysH = "ab\x00cd";
function my011f()
{
   %a = "dup";
   %b = 'dup';
   %c = "fn only";
   %d = myidentvalue;
   return "dup" @ 'dup';
}
