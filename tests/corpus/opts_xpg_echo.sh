# mode: bash
# `xpg_echo` makes `echo` expand escapes without `-e`; `-E` still turns them off.
shopt -s xpg_echo
echo "a\tb"
echo -E "c\td"
echo -n "e\cf"
echo
echo "g\c"
echo h
shopt -u xpg_echo
echo "i\tj"
set -ET
echo "$-"
