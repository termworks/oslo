# mode: bash
# A here-document delimiter loses only its quoting, the shell's way.
cat <<$'EOF'
1 $HOME stays
EOF
cat <<'E"OF'
2 body
E"OF
cat <<"E\"F"
3 body
E"F
cat <<$"EOF"
4 $HOME stays
EOF
cat <<'A'B
5 body
AB
cat <<E"O"F
6 $HOME stays
EOF
