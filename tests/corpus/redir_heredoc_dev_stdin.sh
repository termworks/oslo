# mode: bash
# A small here-document is a pipe, so `/dev/stdin` opened again reads on from the same stream
# instead of starting the document over; `/dev/stdout`, `/dev/stderr` and `/dev/fd/N` name the
# shell's own descriptors.
paste - /dev/stdin <<A 3<<B
x
A
y
B
cat - /dev/stdin <<'EOF'
only once
EOF
echo out > /dev/stdout
echo err > /dev/stderr 2>&1
exec 3>three.txt
echo three > /dev/fd/3
exec 3>&-
cat three.txt
cat /dev/fd/0 <<< here
