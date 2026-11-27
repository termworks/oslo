# mode: bash
# An EXIT trap run because a fatal signal arrived sees the status the last command left; the
# shell still ends with 128 + the signal.
trap 'echo "EXIT saw $?"' EXIT
true
kill -TERM $$
echo not reached
