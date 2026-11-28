# mode: bash
# `=~` matches bytes: in the C locale `.` is any byte, and a byte that is not UTF-8 matches
# itself. `+=` joins two halves of one character into that character.
x=$'\xff'
[[ $x =~ ^.$ ]] && echo dot
[[ $x =~ $'\xff' ]] && echo literal
[[ a${x}b =~ a(.)b ]] && printf %s "${BASH_REMATCH[1]}" | od -An -tx1
[[ é =~ ^..$ ]] && echo two-bytes
[[ é =~ [é] ]] && echo bracket
[[ aé =~ a(.) ]] && printf %s "${BASH_REMATCH[1]}" | od -An -tx1
LC_ALL=C.UTF-8
[[ $x =~ ^.$ ]] || echo no-dot
[[ é =~ ^.$ ]] && echo one-char
LC_ALL=C
y=$'\xc3'
y+=$'\xa9'
z=$'\xff'
z+=$'\xfe'
a=($'\xc3')
a[0]+=$'\xa9'
LC_ALL=C.UTF-8
echo "${#y} ${#z} ${#a[0]}"
