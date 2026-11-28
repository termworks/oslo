# mode: bash
# A command whose words all expand away takes its last substitution's status.
$(exit 3)
echo "$?"
`exit 5`
echo "$?"
$(exit 4) > /dev/null
echo "$?"
$(true)
echo "$?"
set -e
$(false)
echo not reached
