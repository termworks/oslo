# mode: bash
# `set -u` reaches into arithmetic: an unset plain name is fatal there as anywhere, while an
# array element that is not there is still 0. And `++`/`--` that step no variable are signs.

echo $(( 1 +++ 2 )) $(( 1 --- 2 )) $(( ++5 )) $(( --5 )) $(( 5 -- 2 )) $(( 1 ++ 2 ))
a=1
echo $(( a+++1 )) $a $(( a---1 )) $a
x=3
echo $(( - -x )) $(( -- x )) $x $(( +-+x ))

( set -u; echo $(( nope + 1 )); echo unreachable ) 2>/dev/null; echo "word: $?"
( set -u; (( nope++ )); echo unreachable ) 2>/dev/null; echo "command: $?"
( set -u; ((nope)); echo unreachable ) 2>/dev/null; echo "test: $?"
( set -u; let 'y = nope + 1'; echo unreachable ) 2>/dev/null; echo "let: $?"
( set -u; for ((i = 0; i < nope; i++)); do :; done; echo unreachable ) 2>/dev/null; echo "for: $?"
( set -u; ((y += 1)); echo unreachable ) 2>/dev/null; echo "compound assignment: $?"
( set -u; echo $((x = 1, x + 1)) ) 2>/dev/null; echo "assigned first: $?"
( set -u; a=(1); echo $((a[5])) ) 2>/dev/null; echo "missing element: $?"
( set -u; p=b; b=; echo $((p + 1)) ) 2>/dev/null; echo "empty through a name: $?"
( set -u; p=b; echo $((p + 1)); echo unreachable ) 2>/dev/null; echo "unset through a name: $?"

( set -u; r=u; echo ${!r}; echo unreachable ) 2>/dev/null; echo "indirect to unset: $?"
( set -u; echo ${!u}; echo unreachable ) 2>/dev/null; echo "indirect through unset: $?"
( set -u; r=u; echo ${!r:-d} ) 2>/dev/null; echo "indirect defaulted: $?"
