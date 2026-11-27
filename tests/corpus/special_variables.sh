# mode: bash
for v in BASHPID BASH_ARGV0 BASH_SUBSHELL DIRSTACK GROUPS HISTCMD HOSTTYPE OPTERR PS4 MACHTYPE \
    OSTYPE BASH_EXECUTION_STRING COMP_WORDBREAKS BASH_LOADABLES_PATH EPOCHREALTIME \
    BASH_MONOSECONDS SRANDOM BASH_ARGC BASH_ARGV; do
    [[ -v $v ]] && echo "$v set" || echo "$v unset"
done
echo "[$OPTERR] [$PS4] [$HISTCMD]"
declare -p OPTERR OPTIND HISTCMD BASH_ARGC
echo "${#BASH_ARGV[@]}"
[[ $COMP_WORDBREAKS == $' \t\n"\'@><=;|&(:' ]] && echo wordbreaks
declare -p PPID UID EUID BASHPID SRANDOM RANDOM | sed 's/"[0-9]*"/N/'
[[ ${GROUPS[0]} == "$(id -g)" ]] && echo groups
GROUPS=(1 2); [[ ${GROUPS[0]} == "$(id -g)" ]] && echo groups-kept
BASH_ARGV0=renamed; echo "$0 $BASH_ARGV0"
[[ $BASH_EXECUTION_STRING == *BASH_EXECUTION_STRING* ]] && echo execution-string
OPTERR=0; getopts a o -z; echo "[$o]"
EPOCHSECONDS=5; [[ $EPOCHSECONDS != 5 ]] && echo epoch-kept
[[ $BASH_MONOSECONDS =~ ^[0-9]+$ ]] && echo monotonic
HISTCMD=9; echo "$HISTCMD"
unset SECONDS; SECONDS=2+3; echo "$SECONDS"
