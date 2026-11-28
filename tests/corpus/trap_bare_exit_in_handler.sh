# mode: bash
# A bare `exit` inside a trap handler leaves with the status from before the handler ran.
trap 'false; exit' USR1
(exit 7)
kill -USR1 $$
echo not reached
