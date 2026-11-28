# mode: bash
# The `compatNN` names share one level, which `BASH_COMPAT` also sets; `BASHOPTS` follows only
# what `shopt` itself changed.
shopt -s compat31
echo "$BASH_COMPAT"
shopt -s compat32
shopt compat31 compat32
shopt -u compat32
echo "$BASH_COMPAT"
BASH_COMPAT=42
shopt compat42
BASH_COMPAT=4.3
shopt compat43 compat42
echo "$BASHOPTS"
unset BASH_COMPAT
shopt compat43
shopt -s compat44
echo "$BASH_COMPAT $BASHOPTS"
shopt -u compat44
echo "$BASH_COMPAT"
shopt -s login_shell
shopt login_shell
