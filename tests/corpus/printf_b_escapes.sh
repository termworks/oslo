# mode: bash
# `%b` reads `\0NNN` with up to three digits after the zero, keeps the backslash on `\"` and
# `\?`, and a `\c` in it ends all output, the rest of the format included.
printf '%b|' 'a\tb' '\x41' '\0101' '\101' '\1234' '\"\?' '\e[0m' 'a\'; echo
printf '%b\n' 'abc\cdef'; echo after
printf '%s %b %s\n' x 'y\cz' w; echo "[rc=$?]"
printf 'A%bB\n' '\c'; echo
printf '%5b|%-5b|%.1b|%.3b|\n' ab ab abc 'a\tbc'
printf '%d%b' abc '\c' 2>/dev/null; echo " rc=$?"
printf '%b' '\0033[0m\x7' | od -An -tx1
