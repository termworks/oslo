# mode: bash
# With `lastpipe` the last stage of a pipeline runs in the shell itself.
shopt -s lastpipe
echo x | read v
echo "[$v]"
n=0
printf 'a\nb\n' | while read -r l; do n=$((n+1)); done
echo "$n"
true | false
echo "$?"
false | true
echo "${PIPESTATUS[@]}"
