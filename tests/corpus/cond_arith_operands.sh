# mode: bash
# Inside `[[ ]]` the operands of -eq, -ne, -lt, -le, -gt and -ge are arithmetic expressions;
# `test` and `[` read decimal integers only.

n=
[[ $n -gt 0 ]]; echo "empty -gt 0: $?"
[[ $n -eq 0 ]]; echo "empty -eq 0: $?"
x=3
[[ x -eq 3 ]]; echo "name: $?"
[[ 1+1 -eq 2 ]]; echo "sum: $?"
[[ 010 -eq 8 ]]; echo "octal: $?"
[[ 0x10 -eq 16 ]]; echo "hex: $?"
[[ 2#101 -eq 5 ]]; echo "base: $?"
[[ " 3 " -eq 3 ]]; echo "blanks: $?"
[[ '1 + 2' -eq 3 ]]; echo "spaced expression: $?"
[[ -1 -lt 0 ]]; echo "negative: $?"
y=z z=7
[[ y -eq 7 ]]; echo "chained name: $?"
i=2
[[ i++ -eq 2 ]]; echo "side effect: $? i=$i"
[[ 08 -eq 8 ]] 2>/dev/null; echo "bad octal: $?"
[[ 08 -eq 8 ]] 2>/dev/null || echo "bad octal is false"
[[ 1 -eq 1/0 ]] 2>/dev/null; echo "division by zero: $?"
[[ a+ -eq 1 ]] 2>/dev/null; echo "syntax error: $?"

[ 1+1 -eq 2 ] 2>/dev/null; echo "test sum: $?"
[ 010 -eq 8 ]; echo "test octal-looking: $?"
[ 010 -eq 10 ]; echo "test decimal: $?"
[ 08 -eq 8 ]; echo "test leading zero: $?"
[ 0x10 -eq 16 ] 2>/dev/null; echo "test hex: $?"
[ "" -eq 0 ] 2>/dev/null; echo "test empty: $?"
[ " 3 " -eq 3 ]; echo "test blanks: $?"
[ x -eq 1 ] 2>/dev/null; echo "test name: $?"

( set -u; [[ nope -eq 0 ]]; echo unreachable ) 2>/dev/null; echo "nounset: $?"
