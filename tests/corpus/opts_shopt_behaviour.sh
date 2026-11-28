# mode: bash
# The shopt options with behaviour a script can see.
d=$(mktemp -d)
mkdir "$d/a" "$d/b"
printf '#!/bin/sh\necho A\n' > "$d/a/t"
printf '#!/bin/sh\necho B\n' > "$d/b/t"
chmod +x "$d/a/t" "$d/b/t"
PATH=$d/a:$d/b:$PATH
t
rm "$d/a/t"
t 2>/dev/null
echo "stale hash: $?"
shopt -s checkhash
t
echo "checkhash: $?"
shopt -s cdable_vars
target=$d
cd target > /dev/null && [ "$PWD" = "$d" ] && echo "cdable_vars"
touch .hidden
echo .*
shopt -u globskipdots
echo .*
shopt -s globskipdots
cd /
f() { local x; echo "[${x-unset}]"; }
x=global
shopt -s localvar_inherit
f
shopt -u localvar_inherit
v=abc
shopt -u patsub_replacement
echo "${v/b/[&]}"
shopt -s patsub_replacement
echo "${v/b/[&]}"
shopt -s shift_verbose
set -- one
shift 2 2>/dev/null
echo "shift_verbose: $?"
set -u
n=nothing_here
( echo "${!n}" ) 2>/dev/null || echo "indirect unbound"
rm -rf "$d"
