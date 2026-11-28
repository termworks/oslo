# mode: bash
# A declaration builtin reads an array literal the way a bare assignment does: explicit indices,
# quoting, and a literal that spans lines.
declare -a a=([0]=x [2]=y)
echo "1 ${a[@]} ${!a[@]}"
f() { local -a b=([3]=x); echo "2 ${b[3]} ${!b[@]}"; }
f
declare c=([1]=x)
echo "3 ${c[1]} ${!c[@]}"
typeset -a m=(
  [1]=''
  [2]=b # a comment inside
)
echo "4 ${#m[@]} ${m[2]} ${!m[@]}"
declare -a d=(p [5]=q r)
echo "5 ${!d[@]} ${d[6]}"
i=2
declare -a e=([i+1]="three" [$i]=two)
echo "6 ${e[3]} ${e[2]}"
