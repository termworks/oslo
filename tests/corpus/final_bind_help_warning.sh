# mode: bash
# `bind` warns that line editing is off before it reads any option, `--help` included.
bind --help 2>&1 >/dev/null | grep -c 'line editing not enabled'
bind --help >/dev/null 2>&1
echo "rc=$?"
