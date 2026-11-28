# mode: bash
# A here-document the script ends inside is a warning, and its body is everything that is left.
echo before
cat <<EOF; echo after
the body
runs to the end
