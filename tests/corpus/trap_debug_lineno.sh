# mode: bash
# The DEBUG trap sees the line of the command it fires for, not the line of its own text.
trap 'echo "D $LINENO"' DEBUG
a=1
b=2
trap - DEBUG
