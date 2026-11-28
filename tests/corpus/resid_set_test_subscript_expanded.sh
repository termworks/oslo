# mode: bash
q=(a b); f=1; e1=1
[[ -v 'q[e$f]' ]]; echo "dbl $?"
e1=5
[[ -v 'q[e$f]' ]]; echo "lit $?"
test -v 'q[e$f]'; echo "test $?"
[ -v 'q[e1-4]' ]; echo "bracket $?"
declare -A m=([k1]=v); k=k1
[[ -v 'm[$k]' ]]; echo "assoc $?"
e1=1
printf -v 'q[e$f]' X; declare -p q
read 'q[e$f]' <<< Y; declare -p q
unset 'q[e$f]'; declare -p q
declare 'q[e$f]=Z'; declare -p q
[[ -v a[1/0] ]]
echo never
