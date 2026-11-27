# mode: bash
# A `(` group inside `[[ ]]` closes on its own `)`, with or without spaces inside it.
[[ x && (a == a) ]] && echo 1
[[ (abc =~ b) ]] && echo 2
[[ (a == a) && (b == b) ]] && echo 3
[[ ! (x == y) ]] && echo 4
[[ !(a == b) ]] && echo 5
[[ !(a == a) ]] || echo 6
[[ ( -n x ) ]] && echo 7
[[ x == x && (y) ]] && echo 8
[[ ab == !(x) ]] && echo 9
