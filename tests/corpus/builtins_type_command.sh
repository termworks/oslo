# mode: bash
# `type` and `command -v` keep a relative path as written, `command -v` succeeds when any name is
# found, and `command -V` on a function prints its body.
printf '#!/bin/sh\necho ran\n' > x
chmod +x x
type -t ./x
type ./x
type -p ./x
command -v ./x
command -V ./x
command -v echo nosuch
echo "any rc=$?"
command -v nosuch
echo "none rc=$?"
command -V nosuch echo 2>/dev/null
echo "verbose rc=$?"
g() { echo hi; }
command -V g
type g
