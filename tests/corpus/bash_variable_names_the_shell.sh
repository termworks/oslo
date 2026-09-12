# mode: bash
# `$BASH` names a shell a script can start. Unset, it expanded to the empty word and `exec "$BASH"`
# reported `exec: : not found` with status 127.
[ -n "$BASH" ] && echo "named"
"$BASH" -c 'echo child ran'
exec "$BASH" -c 'echo re-exec ran'
