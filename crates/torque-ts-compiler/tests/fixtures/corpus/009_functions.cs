function my009a() {}
function my009b(%a) { return; }
function my009c(%a, %b, %c, %d) { return %a @ %b @ %c @ %d; }
function MyNs::my009d(%this, %x) { return %this.f + %x; }
function my009e()
{
   return 1;
}
package MyPkg
{
   function my009a() { return 2; }
   function MyNs::my009d(%this) { Parent::my009d(%this, 1); }
};
$myafter = 1;
