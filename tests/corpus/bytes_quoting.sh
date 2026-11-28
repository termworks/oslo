# mode: bash
# A byte that is not printable text is quoted as an octal escape, so what `declare -p`, `set`,
# `printf %q` and `${x@Q}` print can be read back in.
x=$'\xff'
declare -p x
set | grep -a '^x='
printf '%q\n' "$x"
echo "${x@Q}"
x=$'\xff\xe9a'
printf '%q\n' "$x"
echo "${x@Q}"
a=($'\xff' $'b\xfe')
declare -p a
export e=$'\xfe'
declare -p e
alias q=$'echo \xff'
alias q | od -An -tx1
eval "$(declare -p x)"
printf %s "$x" | od -An -tx1
