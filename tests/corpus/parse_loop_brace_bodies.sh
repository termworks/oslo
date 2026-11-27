# mode: bash
# `for` takes a `{ }` body after its header, and `in` may follow a newline.
for i in 1 2; { echo "a $i"; }
set -- p q
for x
{ echo "b $x"; }
for x in c d
{ echo "c $x"; }
for x in; { echo never; }
for ((i = 0; i < 2; i++)); { echo "d $i"; }
for ((i = 0; i < 2; i++))
{ echo "e $i"; }
for ((i = 0; i < 1; i++)) { echo "f $i"; }
for x in g; { echo "$x"; } > out
cat out
for x
in h i; do echo "$x"; done
f() { for y; { echo "j $y"; }; }
f 1 2
case a in a) for z in k; { echo "$z"; } ;; esac
