# mode: bash
# `time -p` gives the POSIX report, `TIMEFORMAT` shapes the default one, and an empty one turns
# it off. Digits are masked: only the shape is compared.
{ time -p true; } 2>&1 | sed 's/[0-9]/N/g'
{ time -p; } 2>&1 | sed 's/[0-9]/N/g'
TIMEFORMAT='T=%R'
{ time true; } 2>&1 | sed 's/[0-9]/N/g'
TIMEFORMAT='%3lR|%2U|%0S|%1R|%%'
{ time true; } 2>&1 | sed 's/[0-9]/N/g'
TIMEFORMAT=
{ time true; } 2>&1 | wc -c
unset TIMEFORMAT
{ time true; } 2>&1 | sed 's/[0-9]/N/g'
