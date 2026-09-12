# mode: bash
# `**` with globstar, walked the way bash 5.3 walks it: every entry for a final `**`, directories
# with a trailing slash for `**/`, a named base written with its slash and a matched one without,
# hidden entries skipped unless dotglob, symlinked directories listed but never entered, a missing
# base left as typed, and no match reported twice. oslo returned directories only, walked into
# hidden directories and emitted an empty argument for a bare `**`.
export LC_ALL=C.UTF-8
mkdir -p dir1/sub1/deep1 dir2 .hdir/hsub emptydir loop
touch f.md top.txt dir1/f1.txt dir1/sub1/f2.txt dir1/sub1/deep1/f3.txt dir2/g.md \
    .hdir/h.txt .hdir/hsub/h2.txt .hidden
ln -s dir2 lnk
ln -s top.txt flink
ln -s nowhere dangle
ln -s .. loop/up
shopt -s globstar
printf '[%s]\n' **
printf '[%s]\n' **/
printf '[%s]\n' dir1/**
printf '[%s]\n' dir1/**/
printf '[%s]\n' */**
printf '[%s]\n' **/*.md
printf '[%s]\n' **/d*
printf '[%s]\n' lnk/**
printf '[%s]\n' .hdir/**
printf '[%s]\n' emptydir/**
printf '[%s]\n' nosuch/**
printf '[%s]\n' **/**/*.md
cd dir1 && printf '[%s]\n' ** && cd ..
shopt -s dotglob
printf '[%s]\n' **/*.txt
shopt -u dotglob globstar
printf '[%s]\n' **/*.md
