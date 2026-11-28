# mode: bash
# computed variables are listed as their last read or assignment left them, and none before
names='BASHPID|LINENO|RANDOM|SRANDOM|SECONDS|EPOCHSECONDS|EPOCHREALTIME|BASH_SUBSHELL|BASH_MONOSECONDS|HISTCMD|GROUPS|DIRSTACK|BASH_ARGC|BASH_ARGV|BASH_LINENO|BASH_SOURCE|COMP_WORDBREAKS|BASH_ALIASES|BASH_CMDS|FUNCNAME'
show() { grep -E "^(declare -[-a-zA-Z]+ )?($names)(=|\$)" | sed -E 's/"[0-9]+(\.[0-9]+)?"/"N"/; s/=[0-9]+(\.[0-9]+)?$/=N/'; }
echo "== unread"
declare -p | show
set | show
echo "== after reads"
: "$BASHPID $RANDOM $SRANDOM $SECONDS $EPOCHSECONDS $EPOCHREALTIME $BASH_SUBSHELL $HISTCMD $COMP_WORDBREAKS"
declare -p | show
set | show
echo "== after assignments"
RANDOM=3
SECONDS=4
LINENO=5
declare -p | grep -E '^declare -[-a-zA-Z]+ (RANDOM|SECONDS|LINENO)='
echo "== call stack arrays"
unset BASH_ARGC 2>/dev/null; echo "unset: $?"
BASH_SOURCE=x
declare -p BASH_SOURCE
unset LINENO
echo "lineno after unset: [$LINENO]"
