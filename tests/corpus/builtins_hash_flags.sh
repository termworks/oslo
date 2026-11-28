# mode: bash
# `hash` remembers program locations: `-t` prints them, `-p` sets one, `-d` forgets one, `-l`
# prints the table as commands, and builtins and functions are never hashed. `type` reports a
# remembered command as hashed.
hash -l
hash ls cat sed grep
hash | sed 's|\t.*/|\t|'
hash -l | sed 's| /.*/| |'
hash -t ls | sed 's|.*/||'
f() { :; }
hash f echo
echo "skip rc=$?"
hash
hash -d nosuch 2>/dev/null
echo "delete rc=$?"
hash -r ls
hash | sed 's|\t.*/|\t|'
type ls | sed 's|(.*/|(|'
type -t ls
hash -t 2>/dev/null
echo "terse rc=$?"
hash -p /bin/sh a b
hash -l
hash -t a b
hash -t nosuch 2>/dev/null
echo "missing rc=$?"
hash -r
hash
