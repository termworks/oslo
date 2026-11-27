# mode: bash
# A redirection after `(( ))` or `[[ ]]` belongs to that command.
i=0
(( i++ )) 2>/dev/null
echo "1 $? $i"
[[ a ]] > made
echo "2 $?"
(( 1 )) > also
echo *
(( 0 )) 2>&1 || echo "3 failed"
