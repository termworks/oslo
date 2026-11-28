# mode: bash
# Integer conversions follow C: zero padding goes after the sign, a precision is a minimum digit
# count that turns the `0` flag off, and `#` asks for the base prefix.
printf '%05d|%05d|%+05d|% 05d|%-05d|\n' -7 -42 7 3 -7
printf '%+d|% d|%+ d|%-+5d|%+u|% x|\n' 5 5 5 5 5 5
printf '%.3d|%.0d|%8.3d|%-8.3d|%08.3d|%+.0d|% .0d|\n' 7 0 5 5 5 0 0
printf '%.3x|%.5o|%#o|%#x|%#X|%#08x|%-#8X|%#.0o|%#.0x|%#5.3o|\n' 10 8 8 255 255 255 255 0 0 8
printf "%'d|%'5d|%'.2f\n" 1234567 12 3.14159
printf '%hd|%ld|%lld|%jd|%zd|%td|%hhx\n' 1 2 3 4 5 6 255
printf '%*d|%-*d|%.*d|%0*d|%*d|\n' 5 42 4 7 3 5 6 42 -4 9
