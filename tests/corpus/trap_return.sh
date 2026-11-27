# mode: bash
# A RETURN trap runs as a function or sourced file finishes, in its scope; functions inherit it
# only with `-T`.
f() { local v=inside; trap 'echo "RET v=$v st=$?"' RETURN; echo in; return 4; }
f
echo "out $?"
trap - RETURN
trap 'echo RET-global' RETURN
g() { echo g; }
g
d=$(mktemp -d)
echo 'echo sourced' > "$d/s.sh"
. "$d/s.sh"
set -T
g
rm -rf "$d"
