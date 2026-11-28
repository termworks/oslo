# mode: bash
# `hash -lt` prints each remembered path as the `hash -p` command that would remember it.
hash -p /bin/echo e1
hash -p /bin/true e2
hash -t e1
hash -t e1 e2
hash -lt e1
hash -lt e1 e2
hash -lt e1 nosuch 2>/dev/null
echo "rc=$?"
