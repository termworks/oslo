# mode: bash
# `export -f` hands a function to child shells as `BASH_FUNC_name%%`, and a shell started with
# one in its environment defines it. `$BASH` is the shell running this script, so each shell is
# here its own child.
f() { echo "hi $1"; if true; then echo "body runs"; fi; }
export -f f
"$BASH" -c 'f child'
env | grep -c '^BASH_FUNC_f%%=() {' || true
declare -F | grep ' f$'
# A redefinition keeps the mark and travels with it.
f() { echo "redefined"; }
"$BASH" -c f
# `export -nf` and `unset -f` take it back out of the environment.
export -nf f
env | grep -c '^BASH_FUNC_f%%=' || true
export -f f
unset -f f
env | grep -c '^BASH_FUNC_f%%=' || true
# A definition that arrives in the environment is imported, and stays exported.
env 'BASH_FUNC_g%%=() { echo "imported $*"; }' "$BASH" -c 'g a b; declare -F g; declare -F | grep " g$"'
# An entry that is not a definition is left alone.
env 'BASH_FUNC_h%%=junk' "$BASH" -c 'declare -F h || echo "h is not a function"'
export -f nosuch 2>/dev/null || echo "export -f nosuch: $?"
