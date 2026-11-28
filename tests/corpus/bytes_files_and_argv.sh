# mode: bash
# A value with a byte that is not UTF-8 is that byte as an argument, as a file name, in a
# redirection, and in a glob's result.
x=$'\xff'
echo "$x" > "f$x"
cat f* | od -An -tx1
for f in f*; do printf %s "$f" | od -An -tx1; done
printf '%s\n' "$x" | cat | od -An -tx1
(echo "$x") | od -An -tx1
set -- "$x" b
echo "$@" | od -An -tx1
echo "$*" | od -An -tx1
printf -v v '%s-%s' "$x" "$x"
printf %s "$v" | od -An -tx1
declare -A m
m[$x]=v
printf %s "${!m[@]}" | od -An -tx1
