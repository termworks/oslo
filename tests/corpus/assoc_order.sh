# mode: bash
# `${!m[@]}` walks the keys in the order bash's hash table holds them: 1024 buckets, FNV-1 over
# the key's bytes, each new key at the head of its bucket. A script that prints a map without
# sorting it sees exactly this order, so it is pinned here.
declare -A m=([a]=1 [b]=2 [c]=3)
echo "${!m[@]} / ${m[@]}"
declare -A w=([apple]=1 [banana]=1 [cherry]=1 [date]=1 [elder]=1 [fig]=1 [grape]=1 [x1]=1
  [x2]=1 [x3]=1 [10]=1 [2]=1 [zz]=1 ["a b"]=1 ['x$y']=1 [-]=1 [é]=1 [ü]=1 [日本]=1)
echo "${!w[@]}"
# Assigning an existing key keeps its place; removing and re-adding puts it back at the head.
w[cherry]=9
unset 'w[apple]'
w[apple]=2
echo "${!w[@]}"
# Past 2048 keys the table grows fourfold and every key is re-inserted, still in bash's order.
declare -A big
for ((i = 0; i < 2100; i++)); do declare "big[key$i]=$i"; done
keys=("${!big[@]}")
echo "${#keys[@]} ${keys[*]:0:8}"
echo "${keys[*]: -4}"
