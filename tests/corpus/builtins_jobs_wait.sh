# mode: bash
# `jobs -x` runs a command with job specs replaced by process group ids, and outside POSIX mode
# `wait` answers again for a pid it has already reported.
sleep 0.2 &
jobs -x echo first %1 last | sed 's/[0-9][0-9]*/N/'
jobs -x echo %9
echo "no job rc=$?"
wait
sleep 0.05 &
p=$!
wait "$p" "$p"
echo "twice rc=$?"
wait "$p"
echo "again rc=$?"
(exit 3) &
q=$!
sleep 0.1
wait "$q" "$q"
echo "exit rc=$?"
