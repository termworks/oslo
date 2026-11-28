# mode: bash
# `unset` of a name no variable can have: it may still be a function's, so only `-v` refuses it.
unset 1bad
echo "plain: $?"
unset 'a b'
echo "space: $?"
unset -v 1bad 2>/dev/null
echo "-v: $?"
unset -fv x 2>/dev/null
echo "-fv: $?"
# A variable wins over a function of the same name, and `-f` reaches the function.
both=1
both() { echo "the function"; }
unset both
both
unset both
type both >/dev/null 2>&1 || echo "both gone"
# An element, an associative entry, and every element at once.
a=(1 2 3)
unset 'a[1]'
declare -A m=([k]=v [j]=w)
unset 'm[k]'
declare -p a m
unset 'a[@]'
declare -p a
# `-v` on a nameref removes what it names; `-n` removes the reference.
x=1
declare -n r=x
unset -v r
echo "[${x-unset}]"
declare -p r
unset -n r
declare -p r 2>/dev/null || echo "r gone"
