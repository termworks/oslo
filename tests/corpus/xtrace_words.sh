# mode: bash
exec 2>&1
set -x
echo a b
echo 'a b' "c'd" '' x\"y "'"
echo '$x' 'a*' 'tab	x' $'nl\nx' $'\x01\x7f' $'\e'
echo x~y '{a,b}' '[x]' '!' 'a]' '~x' '#x' 'a#' 'a=b' 'a=~' 'a:~b'
echo 'a;b' 'c|d' '&' '<' '>' '(' ')' '^' '`'
echo héllo
x=""; echo "$x" $x end
[ a = b ]; test -n x
set +x
echo quiet
