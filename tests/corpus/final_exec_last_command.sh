# mode: bash
# The last simple command of a subshell, a substitution, a background job or a pipeline stage
# replaces that process rather than becoming its child, so `$PPID` in it is this shell.
me=$$
t() { [ "$1" = "$me" ] && echo "$2 parent=shell" || echo "$2 parent=other"; }
t "$(sh -c 'echo $PPID')" cmdsub
t "$(true; sh -c 'echo $PPID')" cmdsub-list
t "$( (sh -c 'echo $PPID') )" cmdsub-sub
(sh -c 'echo $PPID') > f1; t "$(cat f1)" subshell
(true; sh -c 'echo $PPID') > f1; t "$(cat f1)" subshell-list
(false || sh -c 'echo $PPID') > f1; t "$(cat f1)" subshell-or
(trap 'echo T' EXIT; sh -c 'echo $PPID') > f1; t "$(head -1 f1)" subshell-exit-trap
(sh -c 'echo $PPID' > f1); t "$(cat f1)" subshell-redir
(true; sh -c 'echo $PPID' > f1); t "$(cat f1)" subshell-list-redir
( (sh -c 'echo $PPID') ) > f1; t "$(cat f1)" nested
sh -c 'echo $PPID' > f1 & wait; t "$(cat f1)" async
cat <(sh -c 'echo $PPID') > f1; t "$(cat f1)" procsub
{ sh -c 'echo $PPID'; } > f1; t "$(cat f1)" group
( kill -9 $(sh -c 'echo $PPID') ); echo "killed=$?"
(sh -c 'kill -9 $PPID'); echo "not reached"
