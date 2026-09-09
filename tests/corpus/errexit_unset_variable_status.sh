# mode: bash
# `set -e` unwinds as a failed command and carries that command's status. Under `-c` oslo answered
# 127 for the unset variable, and 127 means "command not found" to make, to a CI runner and to
# `case $? in 127)` — on the standard opening line of a careful script.
echo before
set -eu
echo "${nope}"
echo unreachable
