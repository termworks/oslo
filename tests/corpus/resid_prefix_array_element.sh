# mode: bash
i=1
a[$i]=Y echo hi; echo "rc=$? [${a[*]}]"
b=(1 2) sh -c 'echo "[$b]"'; echo "[${b[*]}]"
f() { echo "f[${c[*]}]"; }
c[1]=q f; echo "[${c[*]}]"
x=a x+=b sh -c 'echo "[$x]"'
