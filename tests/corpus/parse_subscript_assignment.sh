# mode: bash
# A word is an assignment when its subscript holds an expansion, quotes or spaces.
i=1
a[$i]=Y
echo "1 ${a[1]}"
b=(p)
b[${#b[@]}]=y
echo "2 ${b[@]}"
a[$((2))]=z
echo "3 ${a[2]}"
k=3
a["$k"]=w
echo "4 ${a[3]}"
a[$i]+=x
echo "5 ${a[1]}"
a[$i+3]=q
echo "6 ${a[4]}"
a[ $i ]=s
echo "7 ${a[1]}"
a[$(echo 5)]=t
echo "8 ${a[5]}"
a[`echo 6`]=u
echo "9 ${a[6]}"
x=1 a[ $i + 6 ]=v
echo "10 $x ${a[7]}"
c=()
c[${#c[@]}]=one
c[${#c[@]}]=two
echo "11 ${c[@]}"
for j in 8; do a[$j]=loop; done
echo "12 ${a[8]}"
true && a[$((9))]=w
echo "13 ${a[9]}"
echo a[ 1 ]=literal
