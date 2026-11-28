# mode: bash
declare -i x=5
f() { x=1/0; echo same; }
f
echo never
