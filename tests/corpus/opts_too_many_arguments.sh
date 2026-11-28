# mode: bash
# A second operand to `exit` abandons the rest of a `-c` program with status 1.
echo before
exit 1 2
echo not reached
