# mode: bash
# Bracket expressions, POSIX classes, collating symbols and equivalence classes, the leading-dot
# rule, and the order matches come back in under the C locale. `[[.a.]]` and `[[=a=]]` were left
# as literal text; bash names one character with each.
export LC_ALL=C.UTF-8
touch 9num a1 a10 a2 abc Abc ABC b1 B1 'br[ack]et' c.txt d.txt e.TXT _under 'x]y' Zed .hid .hid2
touch -- -dash
printf '[%s]\n' *
printf '[%s]\n' ??
printf '[%s]\n' [ab]*
printf '[%s]\n' [!ab]*
printf '[%s]\n' [^ab]*
printf '[%s]\n' [a-c]*
printf '[%s]\n' [[:upper:]]*
printf '[%s]\n' [[:digit:]]*
printf '[%s]\n' [[:alpha:][:digit:]]*
printf '[%s]\n' [a-]*
printf '[%s]\n' []x]*
printf '[%s]\n' [[.a.]]*
printf '[%s]\n' [[=a=]]*
printf '[%s]\n' .*
printf '[%s]\n' .h*
printf '[%s]\n' *[0-9]
printf '[%s]\n' 'a'*
printf '[%s]\n' "*"
printf '[%s]\n' zz*
printf '[%s]\n' {a,b}*
