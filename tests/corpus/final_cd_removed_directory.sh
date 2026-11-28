# mode: bash
# A directory removed from under the shell: `cd .` warns that getcwd failed and keeps the logical
# path, and `cd ..` still climbs out.
mkdir -p top/gone
cd top/gone
rmdir ../gone
cd .
echo "rc=$? ${PWD##*/top}"
pwd | sed 's|.*/top||'
cd ..
echo "rc=$? ${PWD##*/}"
