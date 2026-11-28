# mode: bash
# A subscript of an associative array is a string wherever it is written: in an assignment, a
# literal, `declare`, `local`, `unset`, `[[ -v ]]`, `read`, `printf -v` and arithmetic.
declare -A m
k='a b'
m[$k]=1; m["c d"]=2; m['e$f']=3; m[g h]=4; m[1+1]=5; m[$k]+=x; m[ s ]=6
declare -p m
i=3
declare -A n
n[i]=x; n[$i]=y; n[i+1]=z
declare -p n
echo "${n[i]} ${n[3]} ${n[i+1]}"
# Literals: blanks inside a subscript, quotes, `+=` and lines with comments.
declare -A l=([a b]=1 ["x"]=2 [$'k\t']=3 [ p ]=4)
declare -p l
declare -A ml=(
  [a b]=1
  ["x y"]=2 # comment
  [$k]=3
  [three]+=3
)
declare -p ml
declare -A q=(["c]d"]=1 [a[1]]=2)
declare -p q
plain=([ab] [c d])
declare -p plain
# `+=` in a literal: an indexed array appends to what the literal built, a map to what it held.
declare -A ap=([k]=old)
ap+=([k]+=new [j]+=x)
declare -p ap
ap=([k]+=z)
declare -p ap
ap=([k]=1 [k]+=2)
declare -p ap
ix=(p q); ix+=([0]+=1 [5]+=2)
declare -p ix
ix=([2]=x [2]+=y [ 4 ]=z [1 + 1]+=w)
declare -p ix
declare -ai sum=([0]=1 [0]+=2)
declare -p sum
f() {
  local -A m
  local k='p q'
  m[$k]=1
  local 'm[r s]=2'
  declare 'm[t u]=3'
  declare -p m
}
f
declare -A u=([a b]=1 [c]=2 [d]=3)
unset 'u[$k]'
declare -p u
unset "u[c]"
declare -p u
declare -A v=([a b]=1 [1+1]=2)
[[ -v v[$k] ]] && echo y1
[[ -v v["a b"] ]] && echo y2
[[ -v v[1+1] ]] && echo y3
[[ -v v[2] ]] || echo n4
declare -A r
read "r[$k]" <<< v
read 'r[c d]' <<< w
printf -v "r[e f]" %s p
printf -v 'r[1+1]' %s q
declare -p r
declare -A ar
ar[x]=1
(( ar[x] += 5 ))
echo "${ar[x]}" $(( ar[x] * 2 ))
for key in 'x y' z; do ar[$key]=1; done
declare -p ar
a=(1)
unset "a[ ]"
declare -p a
b[ ]=x
declare -p b
