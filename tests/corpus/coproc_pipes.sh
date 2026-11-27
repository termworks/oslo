# mode: bash
# `coproc cmd` publishes its two ends in COPROC and its pid in COPROC_PID and `$!`; once it has
# been waited for, the array and the pid variable are gone.
coproc cat
echo "fds=${#COPROC[@]}"
[[ $COPROC_PID == "$!" ]] && echo "pid is \$!"
echo hello >&"${COPROC[1]}"
read -r line <&"${COPROC[0]}"
echo "got=$line"
pid=$COPROC_PID
exec {COPROC[1]}>&-
wait "$pid"
echo "rc=$?"
declare -p COPROC COPROC_PID 2>/dev/null || echo "COPROC is unset"
