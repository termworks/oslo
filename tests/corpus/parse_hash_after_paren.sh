# mode: bash
# A `#` straight after the `)` of a substitution continues the word; it is not a comment.
echo $((1))#x
x=$(echo 1)#frag
echo "$x"
echo `echo 2`#y
echo "$(echo 3)"#z
cat <(echo a)#b 2>/dev/null; echo "status $?"
((1)) # a comment after an arithmetic command
echo done
