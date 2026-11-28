# mode: bash
# Backslashes where an expansion meets quoting: a line continuation inside a name or a `${}`,
# `\}` in a double-quoted payload, and `\"` inside backquotes inside double quotes.

foo=bar
echo $fo\
o
echo "$fo\
o"
echo ${fo\
o} "${fo\
o}" ${foo:-a\
b}
x=y
echo $\
x

x=1
cat <<EOF
$x\
continued
EOF
cat <<EOF
$fo\
o|\$x|\\|a\
b
EOF

echo "${y:-a\"b}" "${y:-a\}b}" ${y:-\}} "${y:-}}"
echo "${y:-\}}" "${y:-\{}" "${y:-\a}" "${y:-\\}" "${y:-\"}" "${y:-\`}" "${y:-\$}"

echo `echo "$x"` `echo \$x` "`echo \"q\"`"
echo "`echo \\\\`" "`echo \"a b\"`"
v="`echo \"inner\"`"; echo $v
echo `echo \"q\"` "`echo '\"'`"
echo 'a\
b' "c\
d"
