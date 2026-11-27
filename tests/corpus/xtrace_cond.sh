# mode: bash
exec 2>&1
set -x
[[ -n a && $x == y* ]]
[[ a == "a*" ]]; [[ x != y ]]; [[ x = y ]]; [[ x != "y" ]]
x="a b"; [[ $x == "$x"* ]]; [[ $x = a\ * ]]
[[ ! -n $x ]]; [[ ! ! -n $x ]]; [[ ! a ]]
[[ -z "" || a < b ]]
[[ "$x" ]]; [[ "" ]]
[[ ( a == a ) && ! b ]]
[[ a == @(a|b) ]]; [[ a == [ab] ]]; [[ 'a]' == 'a]' ]]
[[ x == 'a.b-c+d@e!f(g)h|i{j}k^l$m~n,o/p\q?r*s[t]u%v#w=x:y;z&<>' ]]
[[ x =~ 'a.b-c+d@e!f(g)h|i{j}k^l$m~n,o/p\q?r*s[t]u%v#w=x:y;z&<>' ]]
re='^a'; [[ ab =~ $re ]]; [[ x =~ "a" ]]
y="*"; [[ $x == $y ]]; [[ $x == "$y" ]]
