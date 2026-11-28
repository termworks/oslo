# mode: bash
# `select x; do` offers "$@", PS3 is the prompt, `break` leaves with the body's status, `continue`
# asks again, and an empty word list does nothing at all.
set -- red green blue
PS3='pick> '
select colour; do
    case $REPLY in
        2) continue ;;
    esac
    echo "colour=$colour"
    break
done 2>prompts.txt <<'EOF'
2
3
EOF
echo "rc=$?"
cat prompts.txt
echo
select none in; do echo never; done
echo "empty rc=$?"
f() { select y in "a b" c; do echo "$y"; return 7; done; }
f 2>/dev/null <<< 1
echo "return rc=$?"
