# mode: bash
# An indexed array's subscript is arithmetic text: single quotes and a backslash before an
# ordinary character stay, and the arithmetic refuses them. An associative key is a word.
a=(x y z)
echo "1 ${a['1']}" 2>/dev/null
echo "st=$?"
echo "2 ${a["1"]} ${a[1\
]}"
echo "3 ${#a['1']}" 2>/dev/null
echo "st=$?"
echo "4 ${a[\1]}" 2>/dev/null
echo "st=$?"
echo "5 $(( \1 ))" 2>/dev/null
echo "st=$?"
echo "6 $(( "1" + 1 ))"
i="'1'"
echo "7 ${a[$i]}" 2>/dev/null
echo "st=$?"
declare -A m=(['k']=v)
echo "8 ${m['k']} ${m["k"]} ${m[k]}"
b['1']=q
echo "9 ${b[1]}"
