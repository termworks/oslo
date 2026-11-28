# mode: bash
# A fatal signal that arrives while the shell waits for a command runs the EXIT trap with `$?` as
# the command before it left it, and the shell dies of the signal whatever the trap exits with.
( trap 'echo "T $?"' EXIT; false; p=$BASHPID; false; (kill -TERM $p); echo no )
echo "st=$?"
( trap 'echo "T $?"' EXIT; p=$BASHPID; (exit 3); (kill -TERM $p); echo no )
echo "st=$?"
( trap 'echo "T $?"' EXIT; p=$BASHPID; false; kill -TERM $p; echo no )
echo "st=$?"
( trap 'echo T; exit 7' EXIT; p=$BASHPID; kill -TERM $p; echo no )
echo "st=$?"
( trap 'echo T; exit 7' EXIT; p=$BASHPID; (kill -TERM $p); echo no )
echo "st=$?"
