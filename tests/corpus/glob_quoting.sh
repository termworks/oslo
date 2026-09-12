# mode: bash
# Quoting decides, per character, what a pattern matches — in words, `case`, `[[ ]]` and `${v#p}`.
# A backslash left in an unquoted expansion is an escape (bash 5.2), a partly quoted right side
# of `[[ == ]]` keeps each part's quoting, and a single `=` inside `[[ ]]` matches a pattern.
export LC_ALL=C.UTF-8
touch 'star*name' abc a1
v='star\*n*'
printf '[%s]\n' $v
p='a\*'
case 'a*' in $p) echo y1 ;; *) echo n1 ;; esac
[[ 'a*' == $p ]] && echo y2 || echo n2
[[ abc == "a*"c ]] && echo y3 || echo n3
[[ abc == a'*' ]] && echo y4 || echo n4
[[ abc == a\* ]] && echo y5 || echo n5
q='*'
[[ abc == "$q"c ]] && echo y6 || echo n6
[[ abc = a* ]] && echo y7 || echo n7
[[ abc == $'a*' ]] && echo y8 || echo n8
[[ abc != "a*"c ]] && echo y9 || echo n9
case abc in [[.a.]]*) echo y10 ;; *) echo n10 ;; esac
x=abcabc
echo "${x#*b}" "${x##*b}" "${x%b*}" "${x%%b*}" "${x/b/X}" "${x//b/X}"
