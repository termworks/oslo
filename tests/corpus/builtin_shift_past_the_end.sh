# mode: bash
# needs-bash: 5.3
# Shifting past the end is how a loop over "$@" finds out it is done, so bash says nothing about
# it outside POSIX mode — `builtin_shift.sh` is the same question under `--posix`, where it does.
# A bad operand is a usage error and is numbered apart from it.
set -- a
shift 99
echo "past=$?"
shift abc
echo "usage=$?"
