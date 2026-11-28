# mode: bash
exec 2>&1
set -x
x=1; y="a b"; z=
x=1 true
a=(1 "2 3" $x); a[5]=x; a+=(y); i=1; a[i+1]=q; a[i]+=z
s+=1; s+=" a"
export A=1 B
readonly R=2
m=$'a\tb'; e=$'\x01'; q="it's"
a=1 b=$(echo $a)
