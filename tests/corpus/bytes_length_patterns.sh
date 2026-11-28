# mode: bash
# Each byte that is not UTF-8 is one character to `${#x}` and to a pattern, in the C locale and in
# a UTF-8 one; `${#x}` counts the bytes of a real character only in the C locale.
x=$'a\xffb'
echo "${#x}"
[[ $x == a?b ]] && echo q
[[ $x == a*b ]] && echo star
case $x in a[$'\xff']b) echo bracket ;; esac
[[ $x == *$'\xff'* ]] && echo literal
printf %s "${x//$'\xff'/Q}" "${x:1:1}" "${x#a?}" | od -An -tx1
y=$'\xe2\x94\x80'
echo "${#y}"
LC_ALL=C.UTF-8
echo "${#x} ${#y}"
[[ $y == ? ]] && echo one
z=$'\xc3'$'\xa9'
echo "${#z}"
LC_ALL=C
echo "${#x} ${#y} ${#z}"
