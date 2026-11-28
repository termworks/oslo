# mode: bash
set -u
a=(); echo "empty ${#a[@]}"
b=(1 2); echo "two ${#b[@]}"
declare -A m=([a]=1); unset 'm[a]'; echo "assoc ${#m[@]}"
f() { local -a l; echo "${#l[@]}"; echo same; }
f
echo "after $?"
x=1; echo ${#x[@]}
echo "scalar $?"
echo ${#nope[*]}
echo "end $?"
