# mode: bash
# Floating conversions go through C's long double and print its exact value, rounded half to
# even, so twenty digits of 0.1 are zeros where a double would show its error.
printf '%.20f|%.30f|%.25f\n' 0.1 0.1 1.1
printf '%f|%.0f|%.0f|%.0f|%.1f|%.2f|%#.0f\n' 3.14159 0.5 1.5 2.5 0.25 1.005 2
printf '%e|%.2e|%E|%#.0e|%.0e|%12.3e|%-12.3e|%012.3e|\n' 1234.5 0.000123 1e100 5 5 -1.5 1.5 1.5
printf '%g|%g|%g|%g|%g|%G|%#g|%.3g|%.0g|%g|%.2g\n' 100000 1000000 0.0001 0.00001 1234567 1e-10 1.5 3.14159 7 0 9.96
printf '%f|%f|%F|%e|%5f|%-6f|%05f|%+f|%G\n' inf -inf nan -nan inf inf inf inf nan
printf '%.0f|%f\n' 1e30 -0
printf '%.40f\n' 1e-30
printf '%f\n' 1e5000 2>/dev/null; echo "rc=$?"
printf '%.3f|%.3f|%.3f\n' ' 2' 2. .5
printf '%f\n' 1.5x 2>/dev/null; echo "rc=$?"
