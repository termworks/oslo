# mode: bash
# A foreground command killed by a signal is reported on stderr, as bash reports it.
sh -c 'kill -KILL $$'
echo "status $?"
