# mode: bash
# exec lowers SHLVL outside ( ), keeps it inside one, and puts it back when it fails
lvl() { grep '^SHLVL='; }
SHLVL=5
echo "comsub: $(exec env | lvl)"
(exec env | lvl)
(exec env) | lvl
{ exec env; } | lvl
f() { exec env; }
f | lvl
echo "cleared: $(exec -c env | grep -c SHLVL)"
shopt -s execfail
exec /nonexistent/prog 2>/dev/null
echo "after a failed path: $SHLVL"
exec no-such-program-here 2>/dev/null
echo "after a failed name: $SHLVL"
SHLVL=x
exec /nonexistent/prog 2>/dev/null
echo "renumbered: $SHLVL"
SHLVL=-3
echo "negative: $(exec env | lvl)"
SHLVL=7
env | lvl
cat <(env) | lvl
lvl < <(env)
(env) | lvl
