# mode: bash
echo $BASH_SUBSHELL; (echo $BASH_SUBSHELL; (echo $BASH_SUBSHELL)); echo $(echo $BASH_SUBSHELL)
echo $BASH_SUBSHELL | cat
{ echo $BASH_SUBSHELL; } | cat
f() { echo $BASH_SUBSHELL; }; f | cat; x=$(f); echo $x
echo $( (echo $BASH_SUBSHELL) )
BASH_SUBSHELL=7; (echo $BASH_SUBSHELL)
[[ $$ == "$BASHPID" ]] && echo same
( [[ $$ != "$BASHPID" ]] && echo differ; kill -0 $BASHPID && echo alive )
a=$BASHPID; b=$(echo $BASHPID); [[ $a != "$b" ]] && echo substitution
BASHPID=1; [[ $BASHPID != 1 ]] && echo assignment-ignored
