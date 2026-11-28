# mode: bash
# `a |& b` sends `a`'s standard error down the pipe too, after `a`'s own redirections.
{ echo out; echo err >&2; } |& sort
f() { echo from-f >&2; }
f |& tr a-z A-Z
{ echo kept >&2; } 2>/dev/null |& cat
echo done
