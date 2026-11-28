# mode: bash
# in the C locale every byte of IFS is a separator of its own, for read and for "$*"
LC_ALL=C
IFS=é
read -r a b c <<< "xéyéz"
printf '[%s]' "$a" "$b" "$c" | od -An -tx1
read -r a b <<< "xéyéz"
printf '[%s]' "$a" "$b" | od -An -tx1
read -ra arr <<< "xéyéz"
echo "${#arr[@]}"
IFS=" é"
read -r a b c <<< "  x é yéz  "
printf '[%s]' "$a" "$b" "$c" | od -An -tx1
IFS=é
set -- a b
printf '%s\n' "$*" | od -An -tx1
list=(x y)
printf '%s\n' "${list[*]}" | od -An -tx1
joined=$*
printf '%s\n' "$joined" | od -An -tx1
