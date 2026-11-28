# mode: bash
# `$'…'` keeps its escaped quote when it sits inside `$( )` or `${ }`.
x=$(echo $'it\'s')
echo "1 $x"
x=${unset_here:-$'it\'s'}
echo "2 $x"
echo "3 $(echo $'a\'b')"
echo "4 ${unset_here:-$'tab\there'}"
echo "5 $(echo $'paren )')"
