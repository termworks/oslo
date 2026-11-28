# mode: bash
echo $((1/0)); echo same
echo "next $?"
eval 'echo ${x!!}; echo same'; echo "ev=$?"
f() { echo $((1+)); echo same; }
f; echo fn
echo "after $?"
(echo $((1/0)); echo same); echo "sub=$?"
x=$(echo $((1/0)); echo in); echo "[$x] $?"
v=abc; echo ${v:1/0}; echo same
readonly r; echo ${r:=2}; echo same
echo "end $?"
