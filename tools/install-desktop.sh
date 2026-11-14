#!/usr/bin/env bash
# Adds the editor and the mod manager to the application menu of the current
# user: desktop entries in $XDG_DATA_HOME/applications and icons in
# $XDG_DATA_HOME/icons/hicolor (~/.local/share by default). Nothing outside
# the home folder is touched and no root rights are needed. Open Mod Manager
# is offered for mod files (.zip) and old toolkit patches (.vpat, a file type
# added to $XDG_DATA_HOME/mime) under "Open with".
#
# Usage: tools/install-desktop.sh [--bin-dir DIR]   install or update
#        tools/install-desktop.sh --uninstall       remove what it installed
#
# The programs are looked up in --bin-dir, next to this script (the release
# archive), in target/release and target/debug of the source tree, then in
# PATH. The desktop entries start them by absolute path, so the folder they
# are in must stay where it is (run the script again after moving it). The
# icons are drawn by `doctor-toolkit-gui --write-icons`.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
apps=$data/applications
icons=$data/icons
mime=$data/mime
names=(doctor-toolkit open-mod-manager)
# Names of earlier versions, removed on install and uninstall.
old_names=(doctor-toolkit-mods)

refresh() {
    if command -v update-desktop-database >/dev/null; then
        update-desktop-database -q "$apps" 2>/dev/null || true
    fi
    if command -v update-mime-database >/dev/null && [[ -d $mime/packages ]]; then
        update-mime-database "$mime" 2>/dev/null || true
    fi
    if command -v gtk-update-icon-cache >/dev/null && [[ -f $icons/hicolor/index.theme ]]; then
        gtk-update-icon-cache -q -t -f "$icons/hicolor" 2>/dev/null || true
    fi
}

# Removes the desktop entries and icons of the names given.
remove() {
    local name file
    for name in "$@"; do
        rm -f -- "$apps/$name.desktop" "$mime/packages/$name.xml"
        for file in "$icons"/hicolor/*/apps/"$name".png "$icons"/hicolor/*/apps/"$name".svg; do
            if [[ -e $file ]]; then rm -f -- "$file"; fi
        done
    done
}

uninstall() {
    remove "${names[@]}" "${old_names[@]}"
    refresh
    echo "removed the desktop entries and icons from $data"
}

# Path of program $1.
find_bin() {
    local bin=$1 dir
    for dir in ${bin_dir:+"$bin_dir"} "$here" "$here/../target/release" "$here/../target/debug"; do
        if [[ -x $dir/$bin && -f $dir/$bin ]]; then
            (cd "$dir" && echo "$(pwd)/$bin")
            return
        fi
    done
    command -v "$bin" || true
}

# The file $1 of the desktop entries (an entry template, a file type):
# tools/desktop in the source tree, desktop/ in the release archive.
find_entry() {
    local dir
    for dir in "$here/desktop" "$here/tools/desktop"; do
        [[ -f $dir/$1 ]] && { echo "$dir/$1"; return; }
    done
}

bin_dir=''
case ${1:-} in
--uninstall)
    uninstall
    exit 0
    ;;
--bin-dir)
    bin_dir=${2:?--bin-dir needs a folder}
    ;;
'') ;;
-h | --help)
    sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
*)
    echo "unknown option $1; see --help" >&2
    exit 2
    ;;
esac

gui=$(find_bin doctor-toolkit-gui)
[[ -n $gui ]] || { echo "doctor-toolkit-gui not found; build it or pass --bin-dir" >&2; exit 1; }
mkdir -p "$apps" "$icons"
remove "${old_names[@]}"
"$gui" --write-icons "$icons" >/dev/null
# A user icon theme folder needs an index for the icon cache.
if [[ ! -f $icons/hicolor/index.theme && -f /usr/share/icons/hicolor/index.theme ]]; then
    cp /usr/share/icons/hicolor/index.theme "$icons/hicolor/index.theme"
fi

for name in "${names[@]}"; do
    case $name in
    doctor-toolkit) bin=doctor-toolkit-gui ;;
    *) bin=$name ;;
    esac
    path=$(find_bin "$bin")
    if [[ -z $path ]]; then
        echo "skipping $name: $bin not found" >&2
        continue
    fi
    entry=$(find_entry "$name.desktop")
    [[ -n $entry ]] || { echo "desktop entry $name.desktop not found next to $0" >&2; exit 1; }
    # Exec with the absolute path in double quotes; a path with characters
    # that would need escaping inside them is refused instead.
    case $path in
    *'"'* | *'`'* | *'$'* | *'\'*)
        echo "skipping $name: the path $path has a quote, \$, \` or \\ in it" >&2
        continue
        ;;
    esac
    while IFS= read -r line; do
        if [[ $line == "Exec=$bin "* ]]; then
            line="Exec=\"$path\" ${line#"Exec=$bin "}"
        fi
        printf '%s\n' "$line"
    done <"$entry" >"$apps/$name.desktop.tmp"
    mv -f -- "$apps/$name.desktop.tmp" "$apps/$name.desktop"
    chmod 644 "$apps/$name.desktop"
    echo "installed $apps/$name.desktop -> $path"
    types=$(find_entry "$name.xml")
    if [[ -n $types ]]; then
        mkdir -p "$mime/packages"
        cp -- "$types" "$mime/packages/$name.xml"
    fi
done
refresh
echo "icons in $icons/hicolor; remove everything with: $0 --uninstall"
