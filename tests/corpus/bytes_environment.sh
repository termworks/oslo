# mode: bash
# The environment is bytes both ways: a value with a byte that is not UTF-8 reaches a child as
# that byte, and a shell started with one reads it back intact.
X=$'\xff' env | grep -a '^X=' | od -An -tx1
x=$'\xfe\xe2\x94\x80'
export x
env | grep -a '^x=' | od -An -tx1
"$BASH" -c 'printf %s "$x"; echo " ${#x}"' | od -An -tx1
X=$'a\xffb' "$BASH" -c 'declare -p X'
