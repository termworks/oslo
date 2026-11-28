# mode: bash
# A compound assignment to a declaration builtin is one only as written: `declare a=(1 2)` makes
# an array, while a quoted or expanded `a=(1 2)` makes the scalar `(1 2)` unless `a` is an array
# already or `-a`/`-A` asks for one.
declare 'a=(1 2)'; declare -p a
declare "b=(3 4)"; declare -p b
v="c=(5 6)"; declare $v; declare -p c
declare -a d; declare 'd=(7 8)'; declare -p d
declare e=\(9\); declare -p e
declare e2='(9)'; declare -p e2
local_f() { local 'g=(1 2)' h=(3 4); declare -p g h; }
local_f
declare -A m; declare 'm=([k]=v)'; declare -p m
x=(1 2); declare "x=(a b)"; declare -p x
declare -a 'q=(1 2)'; declare -p q
declare -A 'w=([k]=v)'; declare -p w
v="z=(1 2)"; declare -a "$v"; declare -p z
readonly 'r=(1 2)'; declare -p r
export 'ex=(1)'; declare -p ex
ap=(1); declare 'ap+=(2)'; declare -p ap
s=1; declare 's+=(2)'; declare -p s
declare -a y; declare "y=(\$HOME 2)"; declare -p y | sed "s#$HOME#H#"
t=(1); declare t="(5 6)"; declare -p t
typeset 't2=(1)'; declare -p t2
readonly r2=(1 2); declare -p r2
export x2=(1); declare -p x2
f() { declare 'l=(1 2)' n=(3 4) "o=(5)"; declare -p l n o; }
f
