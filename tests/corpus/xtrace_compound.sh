# mode: bash
exec 2>&1
set -x
x=3; (( x + 1 > 2 )); (( $x * 2 )); ((x))
for i in 1 "a b"; do :; done
for ((i=0;i<2;i++)); do :; done
for ((;;)); do break; done
case x in x) echo hit;; esac
case "$x" in a) ;; *) ;; esac
f() { return 3; }; f || true
true && false || echo or
! false
if true; then echo y; fi
i=1; while (( i > 0 )); do (( i-- )); done
let y=4
{ echo br; }
