# mode: bash
# `%(fmt)T` formats an epoch second in the zone the exported TZ names: no argument or -1 is now,
# -2 is when the shell started, an empty format is %X, and width and precision apply.
export TZ=UTC
printf '%(%Y-%m-%d %H:%M:%S)T\n' 86400
printf '%(%s)T|%(%%)T|%()T|%(%c)T|%(%x)T|%(%A %j)T\n' 5 5 0 0 0 1000000000
printf '%-10(%Y)T|%10.2(%Y)T|%-6(%m)T|%((%Y))T|\n' 0 0 0 0
now=$(printf '%(%s)T' -1); [[ $now =~ ^[0-9]+$ ]] && echo now
plain=$(printf '%(%s)T'); [[ $plain =~ ^[0-9]+$ ]] && echo default
start=$(printf '%(%s)T' -2); [[ $start =~ ^[0-9]+$ ]] && (( now - start < 60 )) && echo start
TZ=XYZ-3 printf '%(%H %Z)T|' 0
export TZ=ABC+5; printf '%(%H %Z)T\n' 0
printf '%(%Y)T\n' abc 2>/dev/null; echo "rc=$?"
printf '%(abc' 1 2>/dev/null; echo " rc=$?"
