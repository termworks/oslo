# mode: bash
# A comment may open a `$( )` or `<( )`, and a `)` inside it is part of the comment.
x=$(# comment with ) inside
echo hi)
echo "1 $x"
x=$(
  # (another) one
  echo there
)
echo "2 $x"
cat <(# a ) note
echo three)
