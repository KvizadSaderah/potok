#!/bin/sh
# Build potok and put it on PATH. Run again after pulling changes.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin="${POTOK_BIN:-$HOME/.local/bin}"

cargo build --release --manifest-path "$here/Cargo.toml"
mkdir -p "$bin"
# Copy to a temporary name, then rename: replacing a running binary in place
# can leave an open potok reading half of the new file.
cp "$here/target/release/potok" "$bin/.potok.new"
chmod 755 "$bin/.potok.new"
mv -f "$bin/.potok.new" "$bin/potok"

echo "installed: $bin/potok ($("$bin/potok" --help | head -1))"
case ":$PATH:" in
  *":$bin:"*)
    found=$(command -v potok 2>/dev/null || true)
    if [ -n "$found" ] && [ "$found" != "$bin/potok" ]; then
      echo "warning: PATH finds $found before $bin/potok"
    fi
    ;;
  *) echo "note: $bin is not on PATH — add it to your shell profile" ;;
esac
