# mode: bash
# BASH_ALIASES and BASH_CMDS mirror the aliases and the remembered commands, and writing them adds
shopt -s expand_aliases
alias ll='ls -l' la=x
declare -p | grep '^declare -A BASH_ALIASES'
echo "${BASH_ALIASES[ll]} ${#BASH_ALIASES[@]}"
declare -p BASH_ALIASES
alias zz=later
set | grep '^BASH_ALIASES='
BASH_ALIASES[hi]='echo hello'
eval hi
BASH_ALIASES+=([yo]='echo yo')
alias yo
unset 'BASH_ALIASES[hi]'
alias hi
BASH_CMDS[mytool]=/nonexistent/mytool
hash -t mytool
declare -p BASH_CMDS
unset 'BASH_CMDS[mytool]'
hash -t mytool
declare -A | grep -c '^declare -A BASH_'
f() { local BASH_ALIASES; declare -p BASH_ALIASES; }
f
unset BASH_ALIASES
declare -p BASH_ALIASES 2>/dev/null; echo "after unset: $?"
alias ll
