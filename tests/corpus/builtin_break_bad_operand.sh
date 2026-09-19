# mode: bash
# needs-bash: 5.3
# A non-numeric count is a usage error in a special builtin: reported, and fatal to a
# non-interactive shell. It used to be an ordinary non-zero status, so the builtin returned into
# the loop it had been asked to leave and complained again on every iteration.
for i in 1 2 3; do break abc; echo body; done
echo NOT_REACHED
