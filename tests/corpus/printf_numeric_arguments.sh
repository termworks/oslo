# mode: bash
# A numeric argument is read as strtoimax/strtold read it: a bad one prints the prefix that did
# parse and fails, an out-of-range one is clamped and fails, and a leading quote is the next
# character's code.
printf '%d\n' 12abc 2>/dev/null; echo "rc=$?"
printf '%d|%d\n' 9223372036854775807 -9223372036854775808; echo "rc=$?"
printf '%d|%d\n' 99999999999999999999 -99999999999999999999 2>/dev/null; echo "rc=$?"
printf '%u|%u|%x|%o\n' 18446744073709551615 -1 -1 -1; echo "rc=$?"
printf '%u\n' 18446744073709551616 2>/dev/null; echo "rc=$?"
printf '%d|%d|%d|%d|%d\n' 0x1f 010 -0x10 ' 12' +5
printf '%d|%d|%d|%d|%x\n' "'a" '"b' "'" "'ab" "'A"
printf '%f|%f|%.1f\n' 0x10 "'a" 0x1.8p1
printf '%d\n' '' 2>/dev/null; echo "rc=$?"
printf '%f\n' '' 2>/dev/null; echo "rc=$?"
printf '%i|%d|%d\n' 3.5 1e3 08 2>/dev/null; echo "rc=$?"
printf '%d|%s|%f|%c|\n'; echo "rc=$?"
printf '%*d|\n' 99999999999 5 2>/dev/null; echo "rc=$?"
printf '%*d|\n' abc 5 2>/dev/null; echo "rc=$?"
