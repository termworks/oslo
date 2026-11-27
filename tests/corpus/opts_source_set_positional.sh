# mode: bash
# `set --` inside `. file args` outlives the file at the top level, but not inside a function;
# `shift` never does. A bare name is searched for on PATH.
d=$(mktemp -d)
printf 'echo "in: $# $*"\nset -- z\n' > "$d/setter.sh"
printf 'shift\n' > "$d/shifter.sh"
printf 'echo from-path\n' > "$d/onpath.sh"
set -- o p q
. "$d/setter.sh" a b
echo "after set: $# $*"
set -- o p q
. "$d/shifter.sh" a b
echo "after shift: $# $*"
f() { . "$d/setter.sh" x; echo "in f: $*"; }
f k
PATH=$d:$PATH
. onpath.sh
shopt -u sourcepath
. onpath.sh 2>/dev/null || echo "not searched"
rm -rf "$d"
