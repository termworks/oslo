# mode: bash
# `set +B` turns brace expansion off, `set -k` makes every assignment-shaped argument part of
# the command's environment, and `set -P` makes `cd` and `pwd` physical.
set +B
echo {a,b} x{1..3}
for i in {1..2}; do echo "item $i"; done
set -B
echo {a,b}
set +o braceexpand
echo "$-" {c,d}
set -o braceexpand
set -k
f() { echo "[$X] $*"; }
f a X=1 b
set +k
d=$(mktemp -d)
mkdir "$d/real"
ln -s "$d/real" "$d/link"
set -P
cd "$d/link"
[ "$PWD" = "$(cd "$d/real" && pwd -P)" ] && echo physical
set +P
cd "$d/link"
[ "$PWD" = "$d/link" ] && echo logical
cd /
rm -rf "$d"
