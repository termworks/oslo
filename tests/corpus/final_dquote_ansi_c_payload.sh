# mode: bash
# Inside double quotes the text of a `$'…'` payload is put back before the `}` is found, so a
# single quote it leaves open makes the expansion a bad substitution.
x= y=v
echo "1 ${x:-a'b'c}"
echo "2 ${x:-$'a\'b\'c'}"
echo "3 ${x:-$'\x7d'}"
echo "4 ${x#$'a\'b'}"
echo "5 ${x/a/$'b\'c'}"
echo "6 ${x:-$'a b'}"
echo ${x:-$'a\'b'}
echo "7 ${y:+$'a\'b'}" 2>/dev/null
echo "st=$?"
echo "8 ${x:-$'a\'b'}" 2>/dev/null
echo "st=$?"
