# mode: bash
# `set` with no operands lists every variable — arrays and maps included — in bash's quoting.
a=(1 "x y")
declare -A m=([k]=v)
x=$'a\nb' y="it's" z="" w="~x" v="a#b" u="#a" t="p:~q" s='$HOME'
declare unset_one
set | grep -E '^(a|m|x|y|z|w|v|u|t|s|unset_one)='
