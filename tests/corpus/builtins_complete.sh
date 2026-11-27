# mode: bash
# `complete` stores specs and prints them back as commands, `compopt` edits their options, and
# `compgen` generates from the same options.
complete -W "a b" -o nospace -F _f foo
complete -d -f -A function -A signal -X "*.o" -P p -S s cmd2
complete -W x -E
complete -W "it's" y
complete -p
complete -p foo
complete -p nosuch 2>/dev/null
echo "missing rc=$?"
compopt foo
compopt -o filenames +o nospace foo
complete -p foo
compopt -o nospace nosuch 2>/dev/null
echo "compopt missing rc=$?"
compopt -o nospace 2>/dev/null
echo "compopt outside rc=$?"
complete -r foo
complete -r foo 2>/dev/null
echo "removed twice rc=$?"
complete -r
complete -p
echo "empty rc=$?"

alias zz=1 aa=2
compgen -W "q b a" -a
compgen -W "ab ac b" -X "ac" a
compgen -W "ab ac b" -X "!ac"
compgen -W "ab ac" -P "<" -S ">" a
x=1
compgen -W "\$x a\ b c"
compgen -X '&b' -W 'ab ax' a
compgen -A nosuch 2>/dev/null
echo "bad action rc=$?"
compgen -W ''
echo "nothing rc=$?"
compgen -k | head -4
compgen -A helptopic | head -5
f() { COMPREPLY=(x y "$1" "$2"); }
compgen -F f wo 2>/dev/null
compgen -C 'echo one two' w 2>/dev/null
compgen -v BASH_VERS
compgen -V found -W "p q"
echo "${found[@]}"
mkdir -p dd/sub
compgen -d dd/
compgen -o plusdirs -W 'aa' d
compgen -b | head -3
