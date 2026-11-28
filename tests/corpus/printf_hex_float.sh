# mode: bash
# `%a` of a long double puts four bits before the point, as glibc does, and rounds the hex
# digits half to even.
printf '%a|%A|%a|%a|%a|%a|%a\n' 1 0.5 0.1 -2.5 1e10 3 0
printf '%.0a|%.1a|%.2a|%#.0a|%10.3A|%-12a|%+a|% a|%015a|\n' 1.999 1.5 1.999 1 0.1 1 1 1 1
printf '%.0a|%.0a|%.0a|%.1a\n' 15.5 1.03125 0.9375 1.96875
printf '%a|%A|%a\n' inf nan -0
