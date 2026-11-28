# mode: bash
set -u
a=(1)
echo "${#a[5]}"
declare -A m=([x]=1)
echo "${#m[k]}"
unset 'a[0]'
echo "${#a[5]}"
declare -a e
echo "${#e[3]}"
echo never
