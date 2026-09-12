# mode: bash
# failglob: a pattern that matches nothing is an error, and the command it was in does not run.
touch a1
shopt -s failglob
echo a*
echo zz*
echo never
