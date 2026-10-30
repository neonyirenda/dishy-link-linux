#!/bin/sh
# Pack the Dishylink GNOME Shell extension for `gnome-extensions install`.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cd "$here/dishylink@dishylink.app"
out="$here/dishylink@dishylink.app.shell-extension.zip"
rm -f "$out"
zip -qr "$out" metadata.json extension.js stylesheet.css icons
echo "$out"
