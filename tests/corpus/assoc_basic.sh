# mode: bash
# Associative arrays: a subscript is a key, not arithmetic.
declare -A m
m[apple]=red
m[banana]=yellow
m[1+1]=sum
echo "${m[apple]} ${m[banana]} ${m[1+1]} [${m[2]}] count=${#m[@]}"
typeset -A t
t[x]=1
echo "typeset: ${t[x]}"
# A literal, `+=` on the whole array and on one entry.
declare -A n=([a]=1 [b]=2)
n+=([c]=3 [a]=9)
n[b]+=x
echo "${n[a]} ${n[b]} ${n[c]} ${#n[@]}"
# The key-value pair form.
declare -A p=(k1 v1 k2 v2)
echo "${p[k1]} ${p[k2]}"
# Keys with spaces and other characters, through the literal and `declare`.
declare -A q=(["a b"]=1 ['c]d']=2 ['e$f']=3)
declare 'q[g h]=4'
echo "${q[a b]}|${q['c]d']}|${q['e$f']}|${q["g h"]}"
# Unsetting one entry, and testing for one.
unset 'q[a b]'
[[ -v 'q[a b]' ]] || echo "gone"
[[ -v 'q[g h]' ]] && echo "present"
test -v "q[c]d]" || echo "not one reference"
# `$m` is the entry whose key is 0, which does not exist here.
echo "[$m] [${m[0]}]"
m=zero
echo "${m[0]} ${#m[@]}"
# Values and keys, sorted so the check does not lean on the walk.
for k in "${!n[@]}"; do echo "$k=${n[$k]}"; done | sort
printf '%s\n' "${n[@]}" | sort | tr '\n' ' '
echo
# An element operator maps over the values; a slice counts positions from 1.
declare -A s=([k]=abc)
echo "${s[@]^^} ${s[k]:1} ${#s[k]}"
# Arithmetic reads a key as written.
declare -A num=([x]=5 [3]=7)
echo $(( num[x] + num[3] ))
(( num[x] += 10 ))
echo "${num[x]}"
