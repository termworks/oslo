# mode: posix
# Under --posix a floating conversion without `L` goes through double, and `%a` uses double's
# layout with one bit before the point.
printf '%.20f|%.20Lf\n' 0.1 0.1
printf '%a|%La|%a|%.1a\n' 1 1 0.1 1.96875
printf '%.17g|%g\n' 0.1 1e300
