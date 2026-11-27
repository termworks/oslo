# mode: bash
# Inside a parenthesised group of a `=~` operand, spaces and shell punctuation are the regex's.
[[ "a=b c" =~ a=([^ ]*) ]] && echo "1 ${BASH_REMATCH[1]}"
c=x
s="a x b"
[[ $s =~ (^| )$c( |$) ]] && echo "2 [${BASH_REMATCH[0]}]"
[[ "a b" =~ (a[ ]b) ]] && echo 3
[[ "a;b" =~ a(;)b ]] && echo 4
[[ "a&b" =~ (a&b) ]] && echo 5
[[ "a<b" =~ a(<)b ]] && echo 6
[[ ab =~ a(b)# ]] || echo 7
[[ ab =~ a|b ]] && echo 8
[[ "a b" =~ a\ (b) ]] && echo 9
[[ ab =~ ^(a)(b)$ ]] && echo "10 ${BASH_REMATCH[2]}"
x=$( [[ ab =~ (a| b) ]] && echo inside ); echo "11 $x"
until [[ a =~ (a| b) ]]; do :; done; echo 12
[[ a =~ ^(a)$ && b =~ (b| c) ]] && echo 13
