# mode: bash
# A list expansion in a context that is never field-split is joined back into one string: `"$@"`
# and `${a[@]}` on a space whatever IFS holds, `$*` and `${a[*]}` on IFS's first character.

set -- a b
a=(a b)
IFS=,

x="$@"; echo "1 $x"
x=$@; echo "2 $x"
x="pre$@post"; echo "3 $x"
x="${a[@]}"; echo "4 $x"
x=$*; echo "5 $x"
x=${a[*]}; echo "6 $x"
x="${@:-q}"; echo "7 $x"
x="${@:1}"; echo "8 $x"
x="${a[@]:0}"; echo "9 $x"
x="${a[@]#z}"; echo "10 $x"
x=${a[@]:0}; echo "11 $x"
x="${!a[@]}"; echo "12 $x"
export X=$@; echo "13 $X"
declare Y=$@; echo "14 $Y"
f() { local Z=$@; echo "15 $Z"; }
f a b

cat <<< "$@"
cat <<< $*
cat <<< "${a[@]:0}"
cat <<EOF
$@|$*|${a[@]}|${a[*]}|${*:1}|${a[@]:0}
EOF

case "$@" in "a b") echo "case: space" ;; "a,b") echo "case: comma" ;; esac
case "${@:1}" in "a b") echo "case slice: space" ;; *) echo "case slice: other" ;; esac
[[ "$@" == "a b" ]] && echo "cond: space"
[[ $@ == "a b" ]] && echo "cond unquoted: space"
[[ "$*" == "a,b" ]] && echo "cond star: comma"
[[ "${a[@]:0}" == "a b" ]] && echo "cond slice: space"

IFS=
x=$@; echo "16 $x"
x=$*; echo "17 $x"
x="${a[@]:0}"; echo "18 $x"
printf '[%s]' $* ${*} ${a[*]} ${*:1}; echo
set -- a '' b
printf '[%s]' $*; echo
for i in $*; do echo "<$i>"; done
[[ $* == ab ]] && echo "cond null ifs: glued"
cat <<EOF
$*|$@|${*:1}
EOF
unset IFS

set --
[[ -n "$@" ]]; echo "empty list is one empty operand: $?"
[[ -e <(true) ]] && echo "process substitution exists"
