# mode: bash
# A background job a signal killed is announced on stderr once it is collected, and `jobs` names
# the signal rather than an exit status.
sleep 5 & kill -9 $!; wait $!; echo "rc=$?"
sleep 5 & kill -USR1 $!; sleep 0.3; jobs; wait; echo "rc=$?"
sleep 5 & kill -TERM $!; wait $!; echo "rc=$?"
sleep 5 & kill -KILL $!; sleep 0.3
jobs
echo done
