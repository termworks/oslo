# mode: bash
echo before
unset 'a[1/0]'; echo "unset=$?"
(a[1/0]=1; echo same); echo "sub=$?"
eval 'a[1/0]=1; echo same'
echo never
