# mode: bash
# `export -p`, `readonly -p`, `local`, `declare -F` and a bare `declare`, in bash's formats.
export EA=1
export EB
export -p | grep -E '^declare -x E[AB]'
export -p EA
readonly RA=1
readonly -a RARR=(1)
readonly -p | grep -E ' R(A|ARR)='
readonly | grep -E ' RA='
f() { local a=1 b; local -i n=2; local; echo ---; local -p; echo ---; local -p a; }
f
f2() { :; }
f3() { :; }
export -f f3
declare -F | grep -E ' f[23]$'
declare -F f2
declare -f -F f2
declare -F f2 nosuch
echo "rc=$?"
declare -f f2
declare -fx | grep -c '^f3 ()'
declare -p -f f3
declare -f f3
declare -f | grep -E '^declare'
x='a b' y=plain z=$'n\nl'
w=(1 2)
declare | grep -E '^(x|y|z|w)='
