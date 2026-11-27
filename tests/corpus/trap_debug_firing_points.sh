# mode: bash
# The DEBUG trap names each command in `$BASH_COMMAND`: simple commands, pipeline stages,
# `for` headers, `case` and `(( ))`. Function bodies are traced only with `-T`.
trap 'echo "D[$BASH_COMMAND]"' DEBUG
x=a
echo a | cat
for i in 1 2; do :; done
case $x in a) :;; esac
((1+2))
f() { true; }
f 1 2
set -T
f
trap - DEBUG
echo "outside: $BASH_COMMAND"
