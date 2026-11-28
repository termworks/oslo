# mode: bash
# `enable -n` turns a builtin off until `enable` turns it back on; `help` prints bash's synopsis
# and short description for each topic it matches.
enable -n echo
enable -n
type -t echo
enable echo
type -t echo
enable -s
enable nosuch 2>/dev/null
echo "nosuch rc=$?"
enable -Z 2>/dev/null
echo "bad option rc=$?"
enable | head -3
help -s cd echo
help -d cd pwd
help pwd | head -2
help -s 'comp*'
help nosuch 2>/dev/null
echo "help rc=$?"
help -s if for
