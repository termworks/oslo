# mode: bash
# Each `!` turns a pipeline's status round again, and two of them leave it un-negated.
! ! false; echo "1 $?"
! ! true; echo "2 $?"
! ! ! true; echo "3 $?"
! ! true | false; echo "4 $?"
set -e
! ! true
echo "5 survived"
! ! false
echo "never"
