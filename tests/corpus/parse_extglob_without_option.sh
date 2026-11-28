# mode: bash
# Without `extglob`, a group outside `[[ ]]` is a syntax error that stops the line before it runs.
echo before; f() { echo @(a|b); }; echo after
