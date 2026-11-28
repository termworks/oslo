# mode: bash
# In an unquoted here-document a line ending in `\` joins the next before the delimiter is
# looked for; a quoted one keeps the backslash and stops at the first delimiter line.
cat <<EOF
1 abc\
EOF
EOF
cat <<EOF
2 a\\
EOF
cat <<EOF
3 a\\\
EOF
EOF
cat <<-EOF
	4 a\
	EOF
	EOF
cat <<'EOF'
5 b\
EOF
cat <<\EOF
6 c\
EOF
