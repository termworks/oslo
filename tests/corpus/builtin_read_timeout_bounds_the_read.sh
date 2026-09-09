# mode: bash
# `-t` bounds the whole read, not each wait for a byte. On a descriptor that always has one ready
# the deadline still has to be consulted, or the read never ends.
read -t 0.3 x < /dev/zero 2>/dev/null
echo "zero=$?"
printf 'hi\n' | { read -t 5 y; echo "line=$? [$y]"; }
