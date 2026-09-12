# mode: bash
# `trap - ERR` puts the condition back, and `trap -p` prints it the way it was set.
trap 'echo caught' ERR
trap -p ERR
false
trap - ERR
false
echo done
