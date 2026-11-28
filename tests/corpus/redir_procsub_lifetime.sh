# mode: bash
# A process substitution in a loop's word list stays open for the whole loop, and every process
# substitution sets `$!`.
for f in <(echo a) <(echo b) <(echo c); do
    cat "$f"
done
cat <(echo hi) >/dev/null
[[ -n $! ]] && echo "bang is set"
while read -r x; do echo "line $x"; done < <(printf '1\n2\n')
