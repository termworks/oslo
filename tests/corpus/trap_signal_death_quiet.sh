# mode: bash
# No report for SIGINT, SIGPIPE, a command substitution, or a pipeline stage before the last.
sh -c 'kill -INT $$'
echo "int $?"
sh -c 'kill -PIPE $$'
echo "pipe $?"
x=$(sh -c 'kill -USR1 $$')
echo "substitution $?"
sh -c 'kill -USR1 $$' | cat
echo "first stage $?"
