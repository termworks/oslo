# mode: bash
# `$_` is the last argument of the command that just ran. `mkdir -p a/b && cd $_` is the idiom;
# unset, `cd` took no argument and went to $HOME, so every later command ran in the wrong place.
true alpha
echo "[$_]"
: one two three
echo "[$_]"
true
echo "[$_]"
f() { :; }
f q
echo "[$_]"
true first
true second
echo "[$_]"
true kept
x=1
echo "[$_]"
mkdir -p deep/dir && cd $_ && basename "$(pwd)"
