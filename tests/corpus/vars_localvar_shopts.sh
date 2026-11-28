# mode: bash
# `shopt -s localvar_inherit` and `localvar_unset` on the scoped variable store: a local starts as
# a copy of what it shadows, nameref aside, and unsetting a caller's local leaves it unset there.
shopt -s localvar_inherit
declare -i n=5; export e=1; a=(1 2); declare -A m=([k]=v); declare -u u=ab; readonly r=1
t=x; declare -n ref=t
f() {
  local n e a m u ref
  declare -p n e a m u ref
  n=2+3; echo "n=$n"
  local t; declare -p t
}
f
declare -p n
g() { local r 2>/dev/null; echo "rc=$?"; }
g
h() { local e=override; printenv e; }
h
printenv e
k() { local a=(9); declare -p a; local m; m[z]=1; declare -p m; }
k
declare -p m
f2() { local n=3+4 u=xy; declare -p n u; }
f2
o() { local v=outer; i; }
i() { local v; echo "i: $v"; }
o
x=g
f3() { local -n x=y; declare -p x; }
y=1
f3
z=(1 2)
f4() { local z=; declare -p z; }
f4
shopt -u localvar_inherit
x=global
inner() { unset x; echo "inner: [${x-unset}]"; declare -p x; }
outer() { local x=outer; inner; echo "outer: [${x-unset}]"; declare -p x; x=again; echo "set: $x"; }
outer; echo "top: $x"
shopt -s localvar_unset
x=global
outer; echo "top: $x"
same() { local x=l; unset x; echo "same: [${x-unset}]"; declare -p x; }
same; echo "top: $x"
x=g
f5() { local -i x=3; g5; declare -p x; }
g5() { unset x; declare -p x; x=1+1; }
f5; echo "$x"
p() { x=1 q; echo "p: $x"; }
q() { unset x; echo "q: [${x-unset}]"; }
p
shopt -u localvar_unset
same
