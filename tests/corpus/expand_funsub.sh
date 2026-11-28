# mode: bash
# bash 5.3's `${ cmd; }` runs in the current shell and stands for its output; `${| cmd; }` runs
# there too and stands for the `REPLY` it left, which is its own.

echo ${ echo hi; } "${ echo a b; }"
x=1
y=${ x=2; echo $x; }
echo "$y $x"
x=${ printf 'a\n\n'; }
echo "[$x]"
x=${ false; }
echo "status: $?"
r=${| REPLY=val; }
echo "[$r]"
f() { REPLY=fromf; }
echo "${| f; }"
REPLY=outer
v=${| echo "inside: [$REPLY]"; REPLY=in; }
echo "$v $REPLY"
g() { echo "printed by g"; REPLY=kept; }
w=${| g; }
echo "$w"
x=${ cd /; }
pwd
echo ${ exit 0; }
echo unreachable
