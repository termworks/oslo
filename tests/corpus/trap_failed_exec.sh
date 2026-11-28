# mode: bash
# With `execfail` a failed `exec` is a status and the shell carries on, its EXIT trap intact.
# Without it the shell ends at once, and the EXIT trap does not run.
d=$(mktemp -d)
mkdir "$d/dir"
shopt -s execfail
trap 'echo "EXIT ran"; rm -rf "$d"' EXIT
exec "$d/nosuch" 2>/dev/null
echo "missing: $?"
exec "$d/dir" 2>/dev/null
echo "directory: $?"
shopt -u execfail
trap 'echo "EXIT must not run"' EXIT
rm -rf "$d"
exec "$d/nosuch" 2>/dev/null
echo not reached
