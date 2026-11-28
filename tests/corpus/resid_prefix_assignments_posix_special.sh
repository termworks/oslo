# mode: posix
f() { :; }
x=1 y=$x :; echo "[$x] [$y]"
x=2 command :; echo "[$x]"
x=3 eval :; echo "[$x]"
x=4 f; echo "[$x]"
x=5 . /dev/null; echo "[$x]"
x=6 export z=$x; echo "[$x] [$z]"
env | grep '^x='
