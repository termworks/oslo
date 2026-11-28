# mode: bash
# `read` drops a NUL that is not its delimiter, `-n` does not count one, `-d ''` splits on it,
# and `mapfile` ends each record at the first NUL in it.
printf 'a\0b\n' | { read -r l; echo "[$l] rc=$?"; }
printf 'a\0bcd\n' | { read -r -n 3 l; echo "[$l]"; }
printf 'a\0bcd\n' | { read -r -N 3 l; echo "[$l]"; }
printf 'x y\0z\n' | { read -r p q; echo "[$p][$q]"; }
printf 'a\0b\0' | { read -r -d '' l; echo "$l"; read -r -d '' l; echo "$l"; }
printf 'a\0b\nc\n' | { mapfile -t a; declare -p a; }
printf 'a\0b\nc\0d' | { mapfile a; declare -p a; }
printf 'a\0b\0c' | { mapfile -d '' a; declare -p a; }
printf 'a\0b\nc\0\n' | while read -r l; do echo "[$l]"; done
printf 'a\0b' | od -An -tx1
