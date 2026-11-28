# mode: bash
# What a substitution captures is bytes: a byte that is not UTF-8 is neither replaced nor lost,
# and `$(<file)` reads a file holding one instead of failing.
x=$(printf '\xe9')
printf %s "$x" | od -An -tx1
echo "${#x}"
x=`printf '\xff\xfe'`
printf %s "$x" | od -An -tx1
x=$(printf '\xff\xfe\n\n')
echo "${#x}"
printf '\xff\n' > f
x=$(<f)
echo "status=$?"
printf %s "$x" | od -An -tx1
x="$(printf '\xc3')$(printf '\xa9')"
echo "${#x}"
printf %s "$x" | od -An -tx1
x=$(printf '\xff'; printf 'a')
printf %s "${x#?}" | od -An -tx1
