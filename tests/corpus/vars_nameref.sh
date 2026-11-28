# mode: bash
# Namerefs: every read and write of the reference is one of the variable it names.
x=1
declare -n r=x
echo "$r"
r=2
echo "$x"
declare -p r x
r+=3
echo "$x ${#r} ${r:1}"
# A function that fills its caller's variable, the idiom namerefs exist for.
setv() { local -n out=$1; out=changed; }
v=orig
setv v
echo "$v"
fill() { local -n into=$1; into=(a b c); }
fill arr
declare -p arr
typeset -n t=x
echo "typeset: $t"
# A reference to an array, and to one element of one.
a=(1 2 3)
declare -n ra=a
echo "${ra[@]} ${#ra[@]} ${ra[2]}"
ra[5]=z
declare -n one='a[1]'
echo "$one"
one=X
declare -p a
declare -A m=([k]=v)
declare -n rm=m
echo "${rm[k]}"
rm[j]=w
echo "${m[j]}"
# `${!ref}` is the name a reference holds; `-R` tests for one.
echo "${!r} ${!one}"
[[ -R r ]] && echo "r is a nameref"
[[ -R x ]] || echo "x is not"
test -R r && echo "test -R"
# A reference with no target yet takes the first assignment as its target.
declare -n late
late=x
declare -p late
# A `for` loop over a nameref points it at each word in turn.
p=1 q=2
declare -n each
for each in p q; do echo "$each"; done
declare -p each
# `unset` acts on the target; `unset -n` on the reference; `+n` makes it a plain variable.
y=5
declare -n ry=y
unset ry
echo "[${y-unset}]"
declare -p ry
z=7
declare -n rz=z
unset -n rz
echo "$z"
declare -p rz 2>/dev/null || echo "rz is gone"
declare -n back=x
declare +n back
declare -p back
# A reference to itself, or to something that is not a name, is refused.
declare -n self=self 2>/dev/null || echo "self: $?"
declare -n bad=1x 2>/dev/null || echo "bad: $?"
# `readonly` and `export` reach through to the target.
w=1
declare -n rw=w
export rw
declare -p w rw
