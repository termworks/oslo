# mode: bash
# A here-document inside `$( )` may hold quotes, a backquote or a `)` without ending anything.
x=$(cat <<EOF
Don't
EOF
)
echo "1 $x"
x=$(cat <<EOF
a "b
EOF
)
echo "2 $x"
x=$(cat <<'EOF'
a `b
EOF
)
echo "3 $x"
x=$(cat <<EOF
a ) b
EOF
)
echo "4 $x"
x="$(cat <<EOF
Don't ) "
EOF
)"
echo "5 $x"
echo "6 $(cat <<EOF
it's ${HOME:+set}
EOF
)"
