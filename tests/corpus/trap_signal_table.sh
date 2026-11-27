# mode: bash
# `trap -l` and `kill -l` print bash's numbered table, and the realtime signals carry bash's
# numbers and names.
trap -l
kill -l
kill -l EXIT 0 RTMIN 34 64 SIGRTMAX-1 int 129 143
trap 'echo rt' RTMIN
trap -p RTMIN
trap - RTMIN
