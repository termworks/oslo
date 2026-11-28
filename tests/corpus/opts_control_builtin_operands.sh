# mode: bash
# Bad operands to the control builtins: bash's status and whether the shell carries on.
exit abc 2>/dev/null
echo "after exit abc: $?"
exit 1x 2>/dev/null
echo "after exit 1x: $?"
f() { return abc; echo not-here; }
f 2>/dev/null
echo "after return abc: $?"
set -- a b c
shift -1 2>/dev/null
echo "after shift -1: $? $#"
shift abc 2>/dev/null
echo "after shift abc: $? $#"
eval -x 'echo hi' 2>/dev/null
echo "after eval -x: $?"
for i in 1; do break 0; done 2>/dev/null
echo "after break 0: $?"
for i in 1; do continue 0; done 2>/dev/null
echo "after continue 0: $?"
