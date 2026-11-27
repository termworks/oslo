# mode: bash
# The format is reused while arguments remain, `--` ends the options, `-v` assigns what would
# have been printed (up to a NUL), and `%n` stores the bytes written so far in the pass.
printf '%s %s %s\n' a b c d e
printf 'x\n' a b c
printf '%s=%d\n' a 1 b
printf -- '%s-' a b; echo
printf -- --; echo
printf -; echo
printf -v x '%s:%d' a 5; echo "$x"
printf -vy '%s' Q; echo "$y"
printf -v z ''; echo "[${z-unset}]"
printf -v w 'a\0b'; echo "${#w}"
printf -v v 'ab%y' 2>/dev/null; echo "rc=$? [$v]"
printf -v u '1%b2' 'x\cy'; echo "rc=$? [$u]"
printf '%n|%s%n|\n' c1 abc c2; echo "[$c1][$c2]"
printf '%s%n\n' ab 1x 2>/dev/null; echo " rc=$?"
printf '' a b; echo "rc=$?"
