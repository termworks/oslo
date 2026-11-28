# mode: bash
# `$@` and `$*` under `-`, `=`, `+` and `?` keep the positionals as a list.

n() { printf '%s:' "$#"; printf '[%s]' "$@"; echo; }

set -- 'a b' c
n "${@:-x}"
n "${@-x}"
n ${@-x}
n "${@:=x}"
n "${@:?m}"
n "${@:+1 2}"
n ${@:+1 2}
n "${*:-x}"

set --
n "${@-a b}"
n "${@+a b}"
n "${*-x}"
n "${*+x}"
n "${@:-}"
n ${@:-a b}

set -- ''
n "${@:+y}" "${*:+z}"
n "${@:-x}"
n "${*:-x}"
n ${@:-x}

set -- '' ''
n "${@:-x}"
n "${*:-y}"
n "${@:+q}"
IFS=
n "${*:-x}"
n "${*:+q}"
n "${@:-x}"
unset IFS

set --
( : "${@:=x}" ) 2>/dev/null; echo "assign to @: $?"
( : "${*:=x}" ) 2>/dev/null; echo "assign to *: $?"
( : "${@:?}" ) 2>/dev/null; echo "error on @: $?"
( : "${1:=x}" ) 2>/dev/null; echo "assign to 1: $?"
f() { ( : "${2:=x}" ) 2>/dev/null; echo "assign to 2: $?"; }
f a
( : "${x:}"; echo unreachable ) 2>/dev/null; echo "empty offset: $?"
x=abc
echo "[${x: }] [${x:1:}] [${x:1: }]"

# Assigning a readonly name through `:=` abandons the command with status 1.
exec 2>/dev/null
readonly r; echo "${r:=2}"; echo unreachable
