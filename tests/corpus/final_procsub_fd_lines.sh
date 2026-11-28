# mode: bash
# Read as a `-c` string, a line that is one simple command keeps its process substitutions open, so
# the next line's take the next descriptor down; a list closes each as it goes, and a function's
# return closes them all.
echo <(true)
echo <(true)
echo <(true) <(true)
cat <(echo a)
echo <(true); echo <(true)
f() { echo <(true); }
f
f
eval 'echo <(true); echo <(true)'
echo <(true)
