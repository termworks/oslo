# mode: bash
x=0
x=1 y=$x sh -c 'echo "ext $x $y"'
f() { echo "fn $x $y"; }
x=2 y=$x f
x=3 y=$x eval 'echo "eval $x $y"'
x=4 y=$x env | grep '^y='
x=5 x=6$x sh -c 'echo "twice $x"'
x=a; x+=b sh -c 'echo "append $x"'
x=7 y=$x export w=$x
echo "after [$x] [$y] [$w]"
x=8 export x; echo "export [$x]"
r=9 readonly r; echo "readonly [$r]"
z=1 declare z; echo "declare [$z]"
g() { w=7 local w; echo "local [$w]"; v=8 local v=9; echo "local [$v]"; }
v=0; g; echo "outer [$v]"
