# mode: bash
# `@K`, `@k`, `@a` and `@A` on associative arrays: keys quoted as `declare -p` quotes them, and a
# space after every pair of `@K`, as bash writes it.
declare -A m=([a]=1 [b]="x y")
printf '[%s]' "${m[@]@K}"; echo
printf '[%s]' "${m[*]@K}"; echo
printf '[%s]' ${m[@]@K}; echo
printf '[%s]' "${m[@]@k}"; echo
printf '[%s]' ${m[@]@k}; echo
printf '[%s]' "${m[*]@k}"; echo
printf '[%s]' "${m[@]@a}" "${m[a]@a}" "${m@a}"; echo
printf '[%s]' "${m[@]@A}"; echo
printf '[%s]' "${m@A}" "${m[a]@A}" "${m[a]@K}" "${m@K}"; echo
declare -A q=(['a b']=1 [$'k\n']=2 [@]=3 [v]='$x' [w]="it's")
printf '[%s]' "${q[@]@K}"; echo
# A copy made from `@K` is the same map.
eval "declare -A copy=(${m[@]@K})"
declare -p copy
# Empty and declared-only maps.
declare -A none
printf '[%s]' "${none[@]@K}" "${none[@]@A}" "${none@A}" "${none@a}"; echo
declare -A empty=()
printf '[%s]' "${empty[@]@K}" "${empty[@]@A}" "${empty@A}" "${empty@a}"; echo
# Attribute letters in bash's order, for every attribute and through a nameref.
declare -iA n=([a]=1)
declare -rA r=([k]=v)
printf '[%s]' "${n@a}" "${r@a}" "${n[@]@A}" "${r@A}"; echo
declare -l l=AB; declare -u u=ab; declare -c c=ab; declare -t t=1; declare -ix ix=2
declare -rx rx=3
printf '[%s]' "${l@a}" "${u@a}" "${c@a}" "${t@a}" "${ix@a}" "${rx@a}"; echo
printf '[%s]' "${l@A}" "${u@A}" "${ix@A}" "${rx@A}"; echo
declare -ai ai=(1 2); declare -ar ar=(1); declare -au au=(a)
printf '[%s]' "${ai@a}" "${ar@a}" "${au@a}" "${ai[@]@a}"; echo
x=1
declare -n ref=x
printf '[%s]' "${ref@a}" "${ref@A}" "${!ref@a}"; echo
a=(1 2)
declare -n ra=a
printf '[%s]' "${ra@a}" "${ra[@]@A}" "${ra@A}"; echo
declare un; declare -x ux
printf '[%s]' "${un@a}" "${ux@a}" "${un@A}" "${ux@A}"; echo
f() { local -i li=1; printf '[%s]' "${li@a}" "${li@A}"; echo; }
f
# Indexed and scalar forms beside them.
e=(); declare -a d
printf '[%s]' "${e[@]@K}" "${e[@]@a}" "${e[@]@A}" "${d[@]@a}" "${d[@]@A}"; echo
s=5; declare -i y=3
printf '[%s]' "${s[@]@A}" "${s[@]@K}" "${y[@]@A}" "${y[@]@a}"; echo
