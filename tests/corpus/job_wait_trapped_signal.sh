# mode: posix
# A trapped signal ends a `wait`: the handler runs and the status is 128 + signo, rather than the
# wait going back to sleep until the child it was watching finishes on its own.
trap 'echo caught' USR1
sleep 2 &
job=$!
sh -c "sleep 0.2; kill -USR1 $$" &
wait $job
echo "wait=$?"
