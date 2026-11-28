# mode: bash
# `$'\xHH'` and `$'\NNN'` are bytes, not code points: a run that spells UTF-8 is that character,
# and any other byte survives as itself. A NUL ends the string.
printf %s $'\xff' | od -An -tx1
printf %s $'\xe2\x94\x80' | od -An -tx1
printf %s $'\377\376' | od -An -tx1
printf %s $'\303\251|\777' | od -An -tx1
printf %s $'\xf4\x8f\xbf\xbf' "$(printf '\xf4\x8f\xbc\x80')" | od -An -tx1
x=$'\xe2\x94'
echo "${#x}"
printf %s "$x" | od -An -tx1
x=$'a\0b'
echo "${#x} $x"
x=$'\xff'
y=${x}z$x
printf %s "$y" | od -An -tx1
echo "$x" | od -An -tx1
echo -e '\xff\xe2\x94\x80' | od -An -tx1
printf '%b' '\xff\0351' | od -An -tx1
