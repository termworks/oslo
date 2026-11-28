# mode: bash
# A number picks an item, anything else leaves the variable empty, an empty line reprints the
# menu, and end of input prints a newline and leaves the loop with status 1. The menu and the
# prompts go to stderr, captured here so their bytes are compared too.
select x in alpha beta gamma; do
    echo "x=[$x] REPLY=[$REPLY]"
done 2>menu.txt <<'EOF'
2

foo
9
 1
EOF
echo "rc=$?"
cat menu.txt
