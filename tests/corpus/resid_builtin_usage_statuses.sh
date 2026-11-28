# mode: bash
source; echo "source $?"
. ; echo "dot $?"
times x >/dev/null; echo "times $?"
times -p >/dev/null; echo "times-p $?"
builtin -p; echo "builtin $?"
builtin -- echo hi
kill -s; echo "kill-s $?"
kill -n; echo "kill-n $?"
shopt -s expand_aliases
alias -- -x='echo dash'
alias
alias -- -x
