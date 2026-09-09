# mode: posix
# `\` before a newline is a line continuation and both characters go, even in the middle of a word
# or an assignment. Every other character is escaped to itself.
P=/usr/bin:\
/usr/local/bin
echo "[$P]"
echo a\
b
echo one \
two
echo "d\
q"
echo \*
echo a\ b
