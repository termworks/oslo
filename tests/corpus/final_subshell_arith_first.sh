# mode: bash
# A subshell whose first command is an arithmetic command: the outer `((` closes apart.
(((1 + 1)); echo x)
echo "st=$?"
(((x = 4)) && echo "x=$x")
echo "x=${x-unset}"
(((0)) || echo zero)
(((1)) )
echo "st=$?"
