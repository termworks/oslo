# mode: bash
# `ulimit -a` and several limits at once print bash's labelled table; a value attached to a limit
# letter is that limit's new value; `umask -S` with a mode sets it and prints it symbolically.
ulimit -a | sed 's/[0-9][0-9]*$/N/'
ulimit -H -a | head -3 | sed 's/[0-9][0-9]*$/N/'
ulimit -n -c | sed 's/[0-9][0-9]*$/N/'
ulimit -c -f | sed 's/[0-9][0-9]*$/N/'
ulimit -nc 2>/dev/null
echo "attached rc=$?"
ulimit -p
ulimit -R | sed 's/[0-9][0-9]*$/N/'
ulimit -p 4 2>/dev/null
echo "pipe rc=$?"
ulimit -T 2>/dev/null
echo "bad rc=$?"
umask -S 027
umask
umask -S 0777
umask -S 0
umask -p 022
umask -pS 022
umask
