# mode: bash
# `read -n` and `-N` count characters in a UTF-8 locale and bytes in the C locale, and a byte that
# is not UTF-8 is one character in either.
printf '\xc3\xa9x' | { read -r -n1 c; printf %s "$c" | od -An -tx1; }
printf '\xe2\x94\x80ab' | { read -r -N2 c; printf %s "$c" | od -An -tx1; }
printf '\xff\xfeab' | { read -r -N2 c; printf %s "$c" | od -An -tx1; }
LC_ALL=C.UTF-8
printf '\xc3\xa9x' | { read -r -n1 c; printf %s "$c" | od -An -tx1; }
printf '\xe2\x94\x80ab' | { read -r -N2 c; printf %s "$c" | od -An -tx1; }
printf '\xff\xfeab' | { read -r -N2 c; printf %s "$c" | od -An -tx1; }
