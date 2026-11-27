#!/usr/bin/env bash
# Refreshes the vendored resource files in core/res/lang from the C# Toolbox tree (while that tree is still kept).
# Since 1.17.0 the native tree is the source of truth: edit core/res/lang directly and run this only to pull a change
# made on the C# side. Usage: tools/sync-lang.sh [path to DomainScheduler]   (prints what changed; commit by hand)
set -euo pipefail
cd "$(dirname "$0")/.."
CS=${1:-../DomainScheduler}
for f in en.json ru.json tables.json threads.ini; do
  src="$CS/src/DomainScheduler.Lib/Lang/$f"
  [ -f "$src" ] || { echo "missing: $src"; exit 1; }
  cmp -s "$src" "core/res/lang/$f" || { cp "$src" "core/res/lang/$f"; echo "updated core/res/lang/$f"; }
done
