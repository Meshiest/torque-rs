function my010f(%cls, %n)
{
   %a = new ScriptObject();
   %b = new ScriptObject(MyName);
   %c = new ScriptObject(MyName2 : MyParent);
   %d = new ScriptObject("My" @ %n, 1, 2);
   %e = new ScriptObject() { f = 1; g[2] = "x"; h = %n; };
   %f = new SimGroup(MyGroup) { new ScriptObject(MyChild) { a = 1; }; new ScriptObject(); };
   %g = new (%cls)();
   %h = new ScriptObject(%n) { datablock = 3; };
   new ScriptObject(MyStmt);
   return %a;
}
datablock MyDataClass(MyData) { a = 1; b = "s"; c[1] = 2; };
datablock MyDataClass(MyData2 : MyData) { a = 2; };
