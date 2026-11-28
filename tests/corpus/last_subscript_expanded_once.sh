# mode: bash
# an indexed subscript is expanded once and then read as arithmetic, never expanded again
set -- 1
a=(x y z)
( echo "${a[\$1]}" ) 2>/dev/null; echo "read: $?"
( echo "${#a[\$1]}" ) 2>/dev/null; echo "length: $?"
( a[\$1]=Q ) 2>/dev/null; echo "assign: $?"
( a['1']=Q ) 2>/dev/null; echo "quoted assign: $?"
( [[ -v a[\$1] ]] ) 2>/dev/null; echo "set test: $?"
test -v "a[\$1]"; echo "test builtin: $?"
echo "${a[$1]} ${a[ $1 + 1 ]} ${a["$1"]}"
echo "blank: ${a[ ]}"
empty=
a[$empty]=E
declare -p a
declare -n r="a[\$1]"
echo "nameref: $r ${#r} ${r^^}"
declare -n all="a[@]"
printf '<%s>' "$all"; echo
declare -A m=(['$1']=lit)
set -- x
[[ -v m[\$1] ]]; echo "assoc set test: $?"
[[ -v "m[\$1]" ]]; echo "assoc quoted set test: $?"
test -v "m[\$1]"; echo "assoc test builtin: $?"
k=1
[[ -v 'a[$k]' ]]; echo "single-quoted subscript: $?"
( [[ -v "a[\$k]" ]] ) 2>/dev/null; echo "escaped in double quotes: $?"
