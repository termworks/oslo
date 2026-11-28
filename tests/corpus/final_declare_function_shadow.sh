# mode: bash
# A function named after a declaration builtin still has its `name=(…)` operands assigned while the
# words expand, in the caller's scope, and it receives the bare name.
declare() { printf '<%s>' "$@"; echo; }
declare f2=(3 4) x=5 'y=(1)' z=(a b) w=()
declare -A m=([k]=v)
declare -i n=(1+1)
typeset -p f2 z w m n
a=(1)
declare a+=(2 3)
typeset -p a
unset -f declare

g() {
    local() { printf '[%s]' "$@"; echo; }
    local q=(7 8)
    typeset -p q
}
g
typeset -p q 2>/dev/null || echo "q is not global"

export() { printf 'e<%s>' "$@"; echo; }
h() { export s=(1 2); }
h
typeset -p s
unset -f export
