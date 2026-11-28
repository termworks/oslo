# mode: bash
# a process substitution as the value of a keyed array element expands to its /dev/fd path
a=([0]=<(echo hi) [1]=<(echo b) x <(:))
declare -p a
a=([0]=x<(:)y [ 1 + 1 ]=">(cat)")
declare -p a
a+=([5]=<(:))
echo "${a[5]}"
declare -A m=([k]=<(:) [a b]=<(:) [c]+=<(:) [d]=plain)
for k in k "a b" c d; do echo "$k=${m[$k]}"; done
declare -a d=([2]=<(:))
declare -p d
f() { local -A l=([x]=<(:)); declare -p l; }
f
