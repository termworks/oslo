# mode: bash
# A slice of a sparse array counts by index, not by position.
a=([1]=x [5]=y [7]=z)
echo "${a[@]:2}|${a[@]:5:1}|${a[@]: -5}|${a[@]:6}|${a[@]:0:2}|${a[@]: -9}"
b=(a b c d)
unset 'b[0]'
echo "${b[@]:1}|${b[@]:0:1}|${b[@]:2:1}|${b[*]: -2}"
# A scalar is element 0 of itself.
x=ab
x+=(c)
declare -p x
c=(1 2)
c+=3
declare -p c
n=5
echo "$((n[0])) $((n[1]))"
n[0]+=1
echo "$n"
m=5
let 'm[0]+=2'
echo "$m"
s=hello
echo "${s[@]:1} ${s[*]:1:2} ${#s[@]} ${s[0]}"
# Negative subscripts count back from the end, and one before the start is an error.
d=(1 2 3)
d[-1]=z
declare -p d
unset 'd[-5]' 2>/dev/null
echo "unset: $?"
echo "[${d[-3]}]"
echo "[${d[-4]}]" 2>/dev/null
# `unset 'a[@]'` empties the array and keeps it.
e=(1 2 3)
unset 'e[@]'
declare -p e
f=(1 2)
unset 'f[*]'
declare -p f
# The whole-array forms of `:?` and `:=`.
g=(1 2)
echo "${g[@]:?empty} / ${g[@]:=x}"
h=()
( : "${h[@]:?is empty}" ) 2>/dev/null
echo "colon-question: $?"
# Appending after a hole goes past the highest index.
i=(1 2 3)
unset 'i[1]'
i+=(4)
declare -p i
j[3]=x
j+=(y)
declare -p j
k=(a b)
k[1]+=c
k[5]+=d
declare -p k
