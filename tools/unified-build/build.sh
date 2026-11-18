#!/usr/bin/env bash
# Builds the unified game tree of The Godfather II: the data of the NTSC disc
# (1.0.766.0, the reference of doctor_toolkit) with the executable 1.0.789.0
# and every language found on the discs.
#
#   base      NTSC disc: text en, es; speech en, es
#   from PAL  text ru, cs, nl, pl; speech ru (GODFATH3.viv); movies ru, cs,
#             nl, pl; fonts with Cyrillic and Central European letters
#
# Usage:
#   tools/unified-build/build.sh --ntsc DIR|ISO --pal ISO --exe FILE --work DIR --out DIR
#
#   --ntsc  the NTSC disc (19.02.2009): its folder or its .iso
#   --pal   the PAL disc (27.01.2009): its .iso
#   --exe   godfather2.exe 1.0.789.0
#   --work  scratch folder (about 5 GB), kept between runs
#   --out   the game folder to make
#   --link  symlink the large files of an NTSC folder instead of copying
#           them (for test benches; the tree then depends on that folder)
#   --pack DIR  also make the pack of `gf2-unify` in DIR: clean
#           trees of both discs are made in the work folder (about 6 GB
#           more for the PAL disc) and compared with the unified tree
#
# Needs: 7z, cabextract, msiinfo (msitools), python3, and doctor-toolkit
# built in target/release (or $TOOLKIT); for --pack also gf2-unify
# (cargo build --release -p doctor-toolkit-unify, or $UNIFY).
#
# What is done to the NTSC data, and why it is enough: both discs hold the
# same 9 069 string ids and byte-identical English texts; the streams of the
# two builds differ by generated ids of world geometry, so they cannot be
# mixed, but text streams, fonts and movies are self-contained.
#   1. text streams of ru, cs, nl, pl are added under the hashes they have
#      on the PAL disc, listed in the three tables of contents as there;
#   2. in gf2/frontend and gf2/globals_global the two fonts and their two
#      atlases are replaced in place by the PAL ones (the atlas keeps the
#      NTSC name hash: the font finds it by that hash), and the language
#      list `lhr` gets the new rows;
#   3. text/localetable.txt lists all languages; es and ru have speech;
#   4. the Russian speech goes into its own archive GODFATH3.viv.
# The game takes the language from `Locale = xx` of
# %LOCALAPPDATA%\EA Games\<title>\settings.txt.
set -euo pipefail

here=$(dirname "$(realpath "$0")")
root=$(dirname "$(dirname "$here")")
toolkit=${TOOLKIT:-$root/target/release/doctor-toolkit}
unify=${UNIFY:-$root/target/release/gf2-unify}
msiinfo=${MSIINFO:-msiinfo}
link=
while (($#)); do
    case $1 in
        --ntsc) ntsc=$2; shift 2 ;;
        --pal) pal=$2; shift 2 ;;
        --exe) exe=$2; shift 2 ;;
        --work) work=$2; shift 2 ;;
        --out) out=$2; shift 2 ;;
        --link) link=--link; shift ;;
        --pack) pack=$2; shift 2 ;;
        *) echo "unified-build: unknown option $1" >&2; exit 2 ;;
    esac
done
: "${ntsc:?--ntsc}" "${pal:?--pal}" "${exe:?--exe}" "${work:?--work}" "${out:?--out}"
die() { echo "unified-build: $*" >&2; exit 1; }
for tool in 7z cabextract python3 "$msiinfo" "$toolkit"; do
    command -v "$tool" >/dev/null || die "$tool is not found"
done
[[ $(sha256sum "$exe" | cut -d' ' -f1) == 31f579a80cfd14f42c0379bbfa5b5b7f151bb06ec5163593023ab5002b1f1286 ]] \
    || die "$exe is not godfather2.exe 1.0.789.0"
mkdir -p "$work" "$out"
work=$(realpath "$work"); out=$(realpath "$out")
quiet() { grep -v -E '^(wrote|backed up|rebuilt|new stream|added) ' || true; }
step() { echo "== $*"; }

# Files of a disc: from its folder, or out of its image.
disc_get() { # disc, destination, patterns...
    local disc=$1 dest=$2; shift 2
    if [[ -d $disc ]]; then
        local p; for p in "$@"; do (cd "$disc" && cp -r --parents -- $p "$dest/"); done
    else
        7z x -y -o"$dest" "$disc" "$@" >/dev/null
    fi
}
viv_cmd() { # disc, number: a command that prints the archive
    if [[ -d $1 ]]; then printf 'cat %q' "$1/GODFATH$2.viv"; else printf '7z x -so %q GODFATH%s.viv' "$1" "$2"; fi
}

# 1. Cabinets and installer tables of both discs.
for ed in ntsc pal; do
    disc=${!ed}; d=$work/$ed
    if [[ ! -f $d/done ]]; then
        step "$ed: cabinets and installer tables"
        mkdir -p "$d/disc" "$d/cab" "$d/idt"
        disc_get "$disc" "$d/disc" autorun.dat 'data_*.cab' Group0.cab
        for t in File Component Directory Media; do "$msiinfo" export "$d/disc/autorun.dat" $t > "$d/idt/$t.idt"; done
        for c in "$d"/disc/*.cab; do cabextract -q -d "$d/cab" "$c"; done
        python3 -I "$here/disc_map.py" "$d/idt" > "$d/map.tsv"
        touch "$d/done"
    fi
done
grep -q $'^text/localetable.txt\t' "$work/ntsc/map.tsv" || die "the map of the NTSC disc is empty"
head -c 64 "$work/ntsc/cab/textLocaletable_txt" | grep -q ntsc_en || die "--ntsc is not the NTSC disc"
head -c 64 "$work/pal/cab/textLocaletable_txt" | grep -q pal_en || die "--pal is not the PAL disc"

# 2. The clean NTSC tree with the executable 1.0.789.0.
step "NTSC tree"
if [[ -d $ntsc ]]; then src=$ntsc; else
    src=$work/ntsc/big
    [[ -f $src/done ]] || { mkdir -p "$src"; 7z x -y -o"$src" "$ntsc" 'GODFATH*.viv' movies GF2.ico GDFBinary.dll >/dev/null; touch "$src/done"; }
fi
python3 -I "$here/make_tree.py" "$work/ntsc/map.tsv" "$work/ntsc/cab" "$src" "$out" $link
rm -f "$out/godfather2.exe"; cp "$exe" "$out/godfather2.exe"
rm -rf "$out/_doctor_toolkit"

# 3. What comes from the PAL disc.
p=$work/pal/parts
if [[ ! -f $p/done ]]; then
    step "PAL: texts, fonts, movies"
    rm -rf "$p"; mkdir -p "$p/frontend" "$p/text"
    "$toolkit" extract -o "$p/frontend" "$work/pal/cab/gf2Frontend_str" >/dev/null
    disc_get "$pal" "$p" movies/ru movies/cs movies/nl movies/pl
    touch "$p/done"
fi
one() { local f; f=$(ls "$p/frontend" | grep -E "$1") || die "no PAL resource $1"; echo "$p/frontend/$f"; }

# Languages in the order of the PAL disc: code, speech code, name, speech
# folder, locale, text stream (ours for en and es, PAL's for the rest).
langs=(
    "en en English (none) en_US dd7d7050"
    "es es Spanish ES es ad8bf024"
    "ru ru Russian RU ru d07a4e4c"
    "cz en Czech (none) cs c9775540"   # the executable 1.0.789.0 turns `cs` into `cz` before the lookup
    "nl en Dutch (none) nl 640f0a78"
    "pl en Polish (none) pl 456ad2f0"
)
step "text streams"
for row in "${langs[@]:2}"; do
    set -- $row; h=$6
    cap=${h^}; t=$p/text/$h; rm -rf "$t"
    "$toolkit" extract -o "$t" "$work/pal/cab/gf2Text${cap}_str" >/dev/null
    f=$(ls "$t"/*); id=$(basename "$f" .bin); id=${id##*_}
    # A GUID that folds (toc.md §3.5) into the hash the PAL disc gives the stream.
    guid=000000000000000000000000${h:6:2}${h:4:2}${h:2:2}${h:0:2}
    "$toolkit" add-stream --direct --guid "$guid" --parent gf2/frontend --toc gf2/frontend.toc \
        --toc gf2/frontend_global.toc --toc gf2/globals_global.toc=gf2/globals_global "$out" "gf2/text/$h" | quiet
    "$toolkit" add-resource --direct --type lh2 --ids "$id" --compress --data "$f" "$out" "gf2/text/$h" | quiet
done

step "fonts and the language list"
python3 -I - "$p" "$(one '_tg4h_bd5b9923')" "$(one '_tg4h_085ea8fc')" "${langs[@]}" <<'EOF'
import struct, sys
p, aachen, impact, rows = sys.argv[1], sys.argv[2], sys.argv[3], [r.split() for r in sys.argv[4:]]
# The atlas header carries the hash of the texture name; the font looks the atlas up by the NTSC one.
for src, name_hash, dst in ((aachen, 0x078BD24A, 'tg4h_aachen.bin'), (impact, 0x75325CFC, 'tg4h_impact.bin')):
    d = bytearray(open(src, 'rb').read()); struct.pack_into('<I', d, 0x14, name_hash); open(f'{p}/{dst}', 'wb').write(d)
service = [['ss', 'en', 'StringIDs', '(none)', 'en_US', 'd70e6b6c'], ['ll', 'en', 'Longest', '(none)', 'en_US', '856c146c']]
signed = lambda h: struct.unpack('<i', struct.pack('<I', int(h, 16)))[0]
lhr = '2\n' + ''.join(f'{r[0]}\ngf2\\text\\{r[5].upper()}\n{signed(r[5])}\n' for r in rows + service)
open(f'{p}/lhr.bin', 'wb').write(lhr.encode())
table = '7\n454107DA\nntsc_en\nNOT SET\nNOT SET\nNOT SET\n'
for r in rows: table += f'{r[0]}\n{r[1]}\n{r[2]}\n1\n{r[3]}\n{r[4]}\n'
for r in service: table += f'{r[0]}\n{r[1]}\n{r[2]}\n0\n{r[3]}\n{r[4]}\n'
open(f'{p}/localetable.txt', 'wb').write((table + '\n').encode())
EOF
for stream in gf2/frontend gf2/globals_global; do
    replace() { # type, id of the NTSC resource, new data
        local index
        index=$("$toolkit" list "$out/$stream.str" | awk -v t="$1" -v i="$2" '$2 == t && index($0, i) { print $1; exit }')
        [[ -n $index ]] || die "$stream has no $1 $2"
        "$toolkit" replace --direct --index "$index" --data "$3" "$out" "$stream" | quiet
    }
    replace ffn e18e3395 "$(one '_ffn_e016b9e0')"   # impact_lt_std
    replace ffn 86860742 "$(one '_ffn_e21a180d')"   # aachen_std_bold
    replace tg4h 078bd24a "$p/tg4h_aachen.bin"
    replace tg4d 078bd24a "$(one '_tg4d_bd5b9923')"
    replace tg4h 75325cfc "$p/tg4h_impact.bin"
    replace tg4d 75325cfc "$(one '_tg4d_085ea8fc')"
    replace lhr e7ca754a "$p/lhr.bin"
done
cp "$p/localetable.txt" "$out/text/localetable.txt"
rm -rf "$out/_doctor_toolkit"

step "movies"
for l in ru cs nl pl; do
    to=$l; [[ $l == cs ]] && to=cz
    mkdir -p "$out/movies/$to"; cp "$p/movies/$l"/* "$out/movies/$to/"
done

step "Russian speech"
if [[ ! -f $work/pal/GODFATH3.viv ]]; then
    python3 -I "$here/build_lang_viv.py" "$work/pal/GODFATH3.viv" ru "$(viv_cmd "$pal" 0)" "$(viv_cmd "$pal" 1)" "$(viv_cmd "$pal" 2)"
fi
rm -f "$out/GODFATH3.viv"
if [[ -n $link ]]; then ln -s "$work/pal/GODFATH3.viv" "$out/GODFATH3.viv"; else cp "$work/pal/GODFATH3.viv" "$out/GODFATH3.viv"; fi

step "check"
"$toolkit" validate "$out" | tail -4
echo "unified-build: $out"

# 4. The pack: what each disc lacks, and the manifest of the unified game.
if [[ -n ${pack:-} ]]; then
    command -v "$unify" >/dev/null || die "$unify is not found"
    step "clean trees of the discs"
    big=$work/pal/big
    [[ -f $big/done ]] || { mkdir -p "$big"; disc_get "$pal" "$big" 'GODFATH*.viv' movies GF2.ico GDFBinary.dll; touch "$big/done"; }
    rm -rf "$work/tree-ntsc" "$work/tree-pal"
    python3 -I "$here/make_tree.py" "$work/ntsc/map.tsv" "$work/ntsc/cab" "$src" "$work/tree-ntsc" --link
    python3 -I "$here/make_tree.py" "$work/pal/map.tsv" "$work/pal/cab" "$big" "$work/tree-pal" --link
    step "pack"
    "$unify" pack "$out" -o "$pack" --ntsc "$work/tree-ntsc" --pal "$work/tree-pal" \
        --ntsc-disc-map "$work/ntsc/map.tsv" --pal-disc-map "$work/pal/map.tsv"
    "$unify" info "$pack"
fi
