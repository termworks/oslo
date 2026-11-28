# mode: bash
# What a shell takes from how it was started: a signal ignored on entry can be neither trapped
# nor reset, `$SHLVL` counts only shells that did not replace another, and each program finds its
# own path in `$_`.
trap '' INT
"$BASH" -c 'trap -p; trap "echo caught" INT; trap - INT; trap -p INT; echo after'
trap - INT
"$BASH" -c 'trap -p; echo none'
a=$SHLVL
b=$("$BASH" -c 'echo $SHLVL')
echo "nested=$(( b - a ))"
c=$("$BASH" -c '"$BASH" -c "echo \$SHLVL"')
echo "replaced=$(( c - a ))"
env | grep -c '^_=.*/env$'
