# mode: bash
# `declare -p` on an associative array: bash's quoting of keys and values, its trailing space,
# and attribute letters in bash's order.
declare -A m=([a]=1 [b]=2 [c]=3)
declare -p m
declare -A q=(["a b"]=1 ['c]d']=2 ['e$f']=3 [@]=4 ['#x']=5 [k~]=6 ['~k']=7)
declare -p q
declare -A ctl=([$'k\n']=$'v\t' [plain]=$'\e[0m')
declare -p ctl
declare -A empty
declare -p empty
empty=()
declare -p empty
declare -iA num=([a]=1+1)
num[b]=3*3
declare -p num
declare -rA ro=([k]=v)
declare -p ro
declare -Ax ex=([k]=v)
declare -p ex
# Assigning a scalar to a map writes the key 0.
declare -A z=([a]=1)
z=x
declare -p z
# The listing a bare `declare` prints uses the same form.
declare | grep -E '^(m|q)='
x=$(declare -p m)
echo "$x"
