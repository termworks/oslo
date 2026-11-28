# mode: bash
# Brace expansion reaches a redirection target, where more than one word is an ambiguous
# redirect; and a letter range passes the punctuation between the cases.

echo hi > out{1..1}x
echo hi > o{a,} 2>/dev/null
ls
echo hi > {a,b} 2>/dev/null; echo "ambiguous: $?"
cat < out{1,}x
ls

printf '[%s]' {Z..a}; echo
printf '[%s]' {a..Z}; echo
echo {A..z} | od -An -c | head -2
