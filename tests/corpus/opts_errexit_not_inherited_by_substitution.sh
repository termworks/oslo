# mode: bash
# Outside POSIX mode a command substitution does not inherit `set -e`, and `$-` says so.
set -e
x=$(false; echo inner)
echo "x=$x"
echo "$(echo $-)"
f() { local y; y=$(false; echo z); echo "y=$y"; }
f
for i in $(false; echo a b); do echo "$i"; done
