# mode: bash
# A function opens no loop for `break` or `continue`, whichever loop it was called from.
f() { break; }
g() { continue; }
for i in 1 2; do echo "$i"; f; done
for i in 1 2; do g; echo "$i"; done
echo "end $?"
