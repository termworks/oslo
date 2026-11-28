# mode: bash
# Without `-E` the ERR trap does not run inside a function; it runs at the call site for the
# function's status. With `-E` it runs in both places.
trap 'echo "ERR ${FUNCNAME[0]:-main} $?"' ERR
f() { false; echo "f went on"; return 1; }
f
g() { false; }
h() { g; echo "h went on"; }
h
k() { local v; v=$(false); echo "k got"; }
k
main() { local tmp=w; trap 'echo "inner [$tmp]"' ERR; false; }
main
trap 'echo "ERR ${FUNCNAME[0]:-main}"' ERR
set -E
g
(false)
echo end
