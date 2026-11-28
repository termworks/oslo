# mode: bash
# `return 1 2`, `shift 1 2` and `break 1 2` abandon the top-level command too.
f() { return 1 2; }
f
echo not reached
