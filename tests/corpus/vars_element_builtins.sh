# mode: bash
# Every builtin that takes a name also takes `name[subscript]`.
declare 'a[1]=x'
declare a[2]=y
declare -p a
f() { local a[1]=z; declare -p a; local -a l=(1 2); local 'l[5]=w'; declare -p l; }
f
printf -v 'p[1]' %s pv
declare -p p
declare -A m
printf -v 'm[k k]' %s mv
read 'm[r r]' <<< "rv"
declare -p m
read r[1] s <<< "one two"
declare -p r s
read 'q[3]' <<< "three"
declare -p q
# `mapfile` without `-O` replaces the array; with it, it keeps the rest.
t=(p q r s)
mapfile -t -O 1 t <<< $'x\ny'
declare -p t
u=(p q r s)
mapfile -t u <<< x
declare -p u
w=(1 2 3)
mapfile -t -O 5 w <<< z
declare -p w
mapfile -t empty < /dev/null
declare -p empty
# A name that is not one is refused, by `read`, `getopts` and `printf -v` alike.
read 1x <<< v 2>/dev/null
echo "read 1x: $?"
read 'x y' <<< v 2>/dev/null
echo "read 'x y': $?"
getopts ab 1x -a 2>/dev/null
echo "getopts 1x: $?"
getopts ab 'o[1]' -a 2>/dev/null
echo "getopts o[1]: $?"
printf -v 1x %s v 2>/dev/null
echo "printf 1x: $?"
declare 'n[x'=1 2>/dev/null
echo "declare n[x: $?"
# And a read-only name is refused with the status bash gives.
readonly ro=1
read ro <<< 5 2>/dev/null
echo "read ro: $?"
getopts ab ro -a 2>/dev/null
echo "getopts ro: $?"
readonly -a rarr=(1)
read -a rarr <<< "p" 2>/dev/null
echo "read -a rarr: $?"
declare -A am
read -a am <<< "a b" 2>/dev/null
echo "read -a am: $?"
# An integer name evaluates what `read` and `printf -v` give it.
declare -i ni
read ni <<< "2+3"
echo "$ni"
printf -v ni '%s' '4*5'
echo "$ni"
declare -l lw
read lw <<< "ABC"
echo "$lw"
