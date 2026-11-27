# mode: bash
# A named coprocess with a group body starts in the background, and starting it succeeds.
coproc counter { echo one; }
echo "started=$?"
