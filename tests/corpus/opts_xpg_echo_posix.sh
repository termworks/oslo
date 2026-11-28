# mode: posix
# In POSIX mode with `xpg_echo`, `echo` reads no options at all.
shopt -s xpg_echo
echo -n "a\tb"
echo -E "c\td"
echo "e"
