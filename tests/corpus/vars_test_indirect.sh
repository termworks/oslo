# mode: bash
# `-v` on an element, on a whole array, and on the special parameters.
arr=(a b)
[[ -v arr[1] ]] && echo y1
[ -v 'arr[0]' ] && echo y2
i=1
[[ -v arr[i] ]] && echo y3
[[ -v 'arr[@]' ]] && echo y4
[[ -v arr[5] ]] || echo n5
[[ -v arr[-1] ]] && echo y6
empty=()
[[ -v 'empty[@]' ]] || echo "empty is unset"
declare -a declared
[[ -v declared ]] || echo "declared is unset"
sparse=([1]=x)
test -v sparse || echo "no element 0"
set -- a b
[[ -v '#' ]] || echo "no #"
[[ -v '@' ]] || echo "no @"
[[ -v 1 ]] && echo "one"
[[ -v 3 ]] || echo "no three"
# An indirection through `a[@]` is every element, as separate words when quoted.
a=(x 'y z')
ref='a[@]'
for e in "${!ref}"; do echo "[$e]"; done
star='a[*]'
for e in "${!star}"; do echo "[$e]"; done
one='a[1]'
echo "${!one}"
count() { echo $#; }
count "${!ref}"
# `${!prefix*}` and `${!prefix@}` list the set variables with that prefix, sorted.
pre_b=1 pre_a=2 pre_B=3
declare pre_unset
pre_arr=(1)
echo ${!pre_*}
for n in "${!pre_@}"; do echo "<$n>"; done
IFS=,
echo "${!pre_*}"
IFS=$' \t\n'
# `${!a[@]}` is the keys of a map and the indices of a sparse array.
s=([3]=x [7]=y)
declare -A km=([k]=1)
echo "${!s[@]} ${!km[@]}"
# A declared name with no value expands to nothing through `${!…}`.
declare dv
name=dv
echo "[${!name}]"
