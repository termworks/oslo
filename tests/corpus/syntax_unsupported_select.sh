# mode: bash
# A select loop reading /dev/null meets end of input at once: it prints a newline and leaves the
# loop with status 1, without running the body.
select choice in one two; do
    echo "picked=$choice"
done
echo "after=$?"
