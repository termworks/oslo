# mode: bash
# `%c` writes one byte, a NUL when the argument is empty or missing, and a precision on `%s` cuts
# bytes. In the C locale `%lc` and `%ls` are the same as `%c` and `%s`.
printf '%c|%c|%c|%5c|%-3c|\n' abc '' é x y | od -An -c
printf '%c' | od -An -c
printf '%.1s|%5s|%.2s|%lc|%5ls|%.1ls|\n' éa é abc é é éa | od -An -c
printf '%s|%5s|%-5s|%.0s|%05s|%.s|\n' ab ab ab ab ab ab
printf '%C|' A; echo
