# mode: bash
# `declare -p` on the variables the shell keeps for itself, with the numbers that change from run
# to run replaced.
declare -p BASH_VERSINFO PPID UID EUID OPTIND OPTERR BASHPID HISTCMD | sed 's/"[0-9][0-9]*"/N/'
declare -p RANDOM SECONDS EPOCHSECONDS SRANDOM BASH_MONOSECONDS EPOCHREALTIME | sed 's/"[0-9.]*"/N/'
declare -p BASH_ARGV BASH_ARGC BASH_LINENO BASH_SOURCE FUNCNAME DIRSTACK BASH_SUBSHELL |
  sed 's/"[0-9][0-9]*"/N/g; s#"/[^"]*"#P#g'
declare -p GROUPS | sed 's/=(.*)/=(…)/'
f() { declare -p FUNCNAME BASH_LINENO; }
f
declare -p FUNCNAME
echo "[${FUNCNAME+set}] [${#FUNCNAME[@]}]"
declare -p MACHTYPE HOSTTYPE OSTYPE IFS PS4 OPTIND
# Assigned, the clock and the generator are ordinary integers no longer.
unset RANDOM
declare -p RANDOM 2>/dev/null || echo "RANDOM gone"
RANDOM=5
declare -p RANDOM
readonly -p | grep -E ' (PPID|UID|EUID|BASH_VERSINFO)=' | sed 's/"[0-9][0-9]*"/N/'
