# mode: bash
# `local x` with no value is local and unset, not empty.
x=global
f() {
  local x
  echo "[${x-unset}]"
  [[ -v x ]] && echo set || echo notset
  declare -p x
  x=1
  declare -p x
}
f
echo "$x"
g() { local u; set -u; echo "$u"; echo "not reached"; }
( g ) 2>/dev/null
echo "set -u fired: $?"
# `unset` of a caller's local reveals what that local hid, by bash's dynamic rules.
outer() { local x=1; inner; echo "outer:${x-unset}"; }
inner() { unset x; echo "inner:${x-unset}"; }
outer
echo "top:$x"
here() { local x=1; unset x; echo "here:${x-unset}"; x=2; echo "here2:$x"; }
here
echo "top:$x"
a() { local x=a; b; echo "a:${x-unset}"; }
b() { local x=b; c; echo "b:${x-unset}"; }
c() { unset x; echo "c:${x-unset}"; }
a
gone() { unset x; echo "gone:${x-unset}"; }
gone
echo "top:${x-unset}"
x=global
# `local -` puts the `set` options back when the function returns.
opts() { local -; set -u; set -o noglob; [[ $- == *u* ]] && echo "in: u"; }
opts
[[ $- == *u* ]] && echo leaked-u || echo restored-u
[[ $- == *f* ]] && echo leaked-f || echo restored-f
# A local takes the export attribute of what it shadows, and a prefix assignment's value.
export E=1
e1() { local E=2; env | grep '^E='; }
e1
e2() { local E; env | grep '^E='; echo "[${E-unset}]"; }
e2
e3() { local -x L=1; env | grep '^L='; }
e3
env | grep -c '^L=' || true
p() { local v; echo "[$v]"; declare -p v; }
v=4 p
# The readonly attribute of a local leaves with it; the builtin's mark on a global stays.
r1() { local -r lr=1; }
r1
lr=5
echo "$lr"
r2() { readonly gr=1; }
r2
gr=5 2>/dev/null
echo "never printed"
echo "$gr"
readonly ro=1
r3() { local ro=2; }
r3 2>/dev/null
echo "local over readonly: $?"
