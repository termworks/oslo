# mode: bash
# `coproc NAME { …; }` names the array; the body talks to the shell over its stdin and stdout,
# and `wait $NAME_PID` reports the status it exits with.
coproc UPPER { while read -r l; do echo "<${l^^}>"; done; exit 3; }
echo "started=$?"
echo one >&"${UPPER[1]}"
read -r reply <&"${UPPER[0]}"
echo "$reply"
pid=$UPPER_PID
eval "exec ${UPPER[1]}>&-"
wait "$pid"
echo "rc=$?"
f() { coproc N { echo; }; coproc cat; }
declare -f f
