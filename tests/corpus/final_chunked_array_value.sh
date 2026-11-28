# mode: bash
# A program read a command at a time still takes an array value across lines, continued or not.
# (the words "no verbose" here make the reader go a command at a time)
a=(1 \
  2
  3)
echo "${a[*]}"
declare -A m=([x]=1 \
  [y]=2)
echo "${m[y]}"
