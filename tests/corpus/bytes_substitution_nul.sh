# mode: bash
# A NUL cannot live in a shell string: a substitution drops each one and says so on stderr, and
# `$(<file)` does the same.
x=$(printf 'a\0b')
echo "$x ${#x}"
printf 'c\0d' > f
y=$(<f)
echo "$y"
z=$(printf '\0\0')
echo "[$z]"
