#!/usr/bin/env bash
# Domain OS Toolbox (native): Rust core tests on Linux, the core as a Windows static library, then the C++ app.
#   ./build.sh            tests + release exe (out/DomainScheduler.exe)
#   ./build.sh --no-test  skip cargo test
#   CARGO_TARGET_DIR=... OBJ=... ./build.sh   own build folders (parallel builds)
#   NO_CORE=1 ./build.sh  the app alone on the core stub (no Rust build, no library)
#   EXTRA_CXXFLAGS=-DDOS_UI_SHOT OBJ=obj-shot OUT=out/shot.exe ./build.sh --no-test   the offscreen screenshot build
set -euo pipefail
cd "$(dirname "$0")"
export RUSTUP_HOME=${RUSTUP_HOME:-$HOME/sdk/rust/rustup} CARGO_HOME=${CARGO_HOME:-$HOME/sdk/rust/cargo}
MINGW=${MINGW:-$HOME/sdk/llvm-mingw-20260922-ucrt-ubuntu-22.04-x86_64/bin}
export PATH=$CARGO_HOME/bin:$MINGW:$PATH
T=x86_64-pc-windows-gnullvm
# The version has one source: core/Cargo.toml [package] version (see core/src/base/version.rs). It also goes into the
# exe's VERSIONINFO (file properties; Check-DomainOS.ps1 reads ProductVersion).
VERSION=$(sed -n '/^\[package\]/,/^\[/s/^version = "\(.*\)"/\1/p' core/Cargo.toml | head -1)
[ -n "$VERSION" ] || { echo "no version in core/Cargo.toml" >&2; exit 1; }
IFS=. read -r V1 V2 V3 <<<"${VERSION%%-*}"
# --locked: the dependency versions are exactly those in Cargo.lock (the release build on GitHub does the same).
if [ -z "${NO_CORE:-}" ]; then
  if [ "${1:-}" != "--no-test" ]; then
    (cd core && cargo test -q --locked --features testsupport)
    (cd threadmap && cargo test -q --locked)
  fi
  (cd core && cargo build -q --locked --release --target $T)
fi
LIB=${CARGO_TARGET_DIR:-$PWD/core/target}/$T/release/libdos_core.a
OBJ=${OBJ:-obj}
OUT=${OUT:-out/DomainScheduler.exe}
mkdir -p out "$OBJ"
# The app links the real core (app/src/dos_core.h, area ffi). NO_CORE=1 builds the app alone on app/src/core_stub.cpp
# (compiled with -DDOS_CORE_STUB); without NO_CORE a library that lacks the ABI is an error, never a silent stub.
STUB=
LIBS=("$LIB")
if [ -n "${NO_CORE:-}" ]; then
  LIBS=(); STUB=-DDOS_CORE_STUB; echo "core: NO_CORE=1, using app/src/core_stub.cpp"
elif ! llvm-nm "$LIB" 2>/dev/null | grep " T dos_app_start$" >/dev/null; then
  echo "core: $LIB does not export the C ABI (dos_app_start)" >&2; exit 1
fi
CXX="x86_64-w64-mingw32-clang++ -std=c++20 -O2 -Wall ${EXTRA_CXXFLAGS:-} -Wno-unused-parameter -municode -DUNICODE -D_UNICODE -DNOMINMAX -D_WIN32_WINNT=0x0A00 -DIMGUI_DEFINE_MATH_OPERATORS $STUB -Iapp/src -Ithreadmap -Iapp/third_party/imgui -Iapp/third_party/imgui/backends"
objs=()
for f in app/third_party/imgui/imgui.cpp app/third_party/imgui/imgui_draw.cpp app/third_party/imgui/imgui_tables.cpp \
         app/third_party/imgui/imgui_widgets.cpp app/third_party/imgui/backends/imgui_impl_dx11.cpp \
         app/third_party/imgui/backends/imgui_impl_win32.cpp $(find app/src -name '*.cpp' | sort); do
  o=$OBJ/$(echo "$f" | tr '/' '_').o
  if [ ! -f "$o" ] || [ "$f" -nt "$o" ] || [ -n "$(find app/src -name '*.h' -newer "$o" 2>/dev/null | head -1)" ]; then $CXX -c "$f" -o "$o"; fi
  objs+=("$o")
done
RES=()
if [ -f app/src/app.rc ]; then
  x86_64-w64-mingw32-windres -Iapp/res -DDOS_V1="$V1" -DDOS_V2="$V2" -DDOS_V3="$V3" -DDOS_VERSION_STR="\\\"$VERSION\\\"" \
    app/src/app.rc -O coff -o "$OBJ/app.res"
  RES=("$OBJ/app.res")
fi
# --no-insert-timestamp: the PE time stamp and build id come from a hash of the content, so the same sources at the
# same path give the same bytes (the attested build on GitHub can be rebuilt and compared).
$CXX -mwindows -static -Wl,--no-insert-timestamp "${objs[@]}" "${RES[@]}" "${LIBS[@]}" \
  -ld3d11 -ldxgi -ld3dcompiler -ldwmapi -luser32 -lgdi32 -lshell32 -lole32 -loleaut32 -luuid -lcomctl32 -limm32 \
  -ladvapi32 -lsetupapi -lcfgmgr32 -lhid -lwinhttp -lbcrypt -lcrypt32 -ltdh -lpowrprof -lversion -lwtsapi32 -lshlwapi -lpsapi -ltaskschd \
  -lkernel32 -lntdll -luserenv -lws2_32 -ldbghelp -lsynchronization -o "$OUT"
llvm-strip "$OUT"
[ -n "${KEEP_OBJ:-}" ] || rm -rf "$OBJ"
echo "$OUT $(stat -c %s "$OUT") bytes, version $VERSION"
