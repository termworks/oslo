# mode: bash
# `declare -p` for every shape, byte for byte as bash prints it.
s='a"b$c`d\e'
declare -p s
ctl=$'a\nb' esc=$'\e[0m' tab=$'tab\there' del=$'\x7f' bell=$'\a\b\f\v\r' q="it's"
declare -p ctl esc tab del bell q
a=($'a\nb' 'c"d')
declare -p a
sparse=([3]=x [1]=y)
declare -p sparse
e=()
declare -p e
declare -a da
declare -p da
# Declared but never given a value.
declare u
declare -x ux
declare -r ur
declare -i ui
declare -n un
declare -p u ux ur ui un
# Attribute letters in bash's order.
declare -rix z=3
declare -xl xl=ABC
declare -il il=1
declare -u uarr
uarr=(abc def)
declare -p z xl il uarr
declare -n nr=s
declare -p nr
typeset -p s
# A name nothing declared is reported, and the others are still printed.
declare -p s nosuch 2>/dev/null
echo "rc=$?"
# Listing by attribute shows only the matching variables.
declare -x LX1=1 LX2=2
LX3=3
declare -x | grep -E '^declare -x LX'
declare -r LR1=1
declare -r | grep -E ' LR'
declare -a | grep -E ' (a|e|sparse|LX1)='
declare -i | grep -E ' (z|il|LX1)='
declare -A am=([k]=v)
declare -A | grep -E ' (am|a)='
