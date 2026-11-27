# DomainScheduler

**English** | [Русский](README.ru.md)

A user-mode Windows utility that tunes how the scheduler places a game's threads and how much
room background programs take. Its goal is **steadier frame times** — better 1% and 0.1% lows
and a more even frame pacing — not a higher average FPS.

Since 1.17.0 it is a native program: the logic is a Rust library, the window is C++20 with Dear ImGui and
DirectX 11, and both are linked into one static `DomainScheduler.exe` of about 8 MB. It keeps the behaviour, the
storage, the command-line switches and the scheduled task of the C# Toolbox 1.16.x (see
[Compatibility with the C# Toolbox](#compatibility-with-the-c-toolbox)).

> **Read this first**
> - **Average FPS does not go up.** The program does not make the CPU or GPU faster. It works on
>   the worst frames: 1% low, 0.1% low, p99 frame time.
> - **Not every PC benefits.** On some builds the difference is within measurement noise, and on
>   some it can be negative. Measure on your own system (see [How to check the effect](#how-to-check-the-effect)).
> - Steadier frame times also mean a more *consistent* input-to-screen delay. The program does not
>   measure or reduce input latency directly.
> - Everything is reverted when the game closes, when you turn it off, and when you exit. After a
>   crash the next start restores what was changed.

## What it does

- **One-button Auto mode.** Finds a game from your watchlist, picks cores for it (cache-aware on
  dual-CCD X3D and hybrid P/E CPUs), raises its priority class to Above normal, gives its key
  threads a preferred core each and lowers background programs.
- **Dynamic Top-5.** The five busiest threads of the game are re-ranked every second and moved
  between cores and priorities as the load changes. (The older "role plan" mode was removed in
  1.16.0 Test 1.9: in the developer's measurements it only made things worse.)
- **Ideal cores for all game threads** (1.17.1, Settings and Game threads page, on). With Auto, every other thread of
  the game (DLL threads too) gets a preferred core: never core 0, never a Top-5 core (its SMT sibling only after every
  quiet core), never a core drivers keep busy (DPC/ISR from the live trace, held 60 s), only P-cores on hybrid CPUs,
  inside the core mask when one is in force. New threads get theirs within about 2 s; a thread moves only for a clear
  reason (3 ticks of hysteresis, 2 s dwell, at most 2 moves per second). Only the ideal processor is written: no
  affinity, no CPU Sets, no priority; the threads still migrate. A thread that enters Top-5 belongs to Top-5; a value
  someone else set is left alone; Auto off puts back only the values that are still Toolbox's own.
- **Watchlist and denylist.** Only programs you mark are tuned. System-critical processes and
  anti-cheat software are never opened or changed.
- **Background layer.** While a watched game runs, other programs get Below-normal priority,
  lowest I/O priority, low memory priority, EcoQoS and (where it makes sense) the non-game cores.
- **Built-in presets** for Windows components and common background apps; your own presets with
  import, export and profiles.
- **Core masks** with a visual CPU map, interrupt-aware refinement and optional core reservation.
- **Devices page**: interrupt pinning and the automatic layout (writing is open since 1.16.0 and asks for confirmation first).
- **Frame-time watchdog.** If p99 frame time gets worse after the changes, they are taken off,
  re-measured, and kept off if the regression is real.
- **Check page** (1.16.0 Test 1.1): one 30-second run while you play — FPS, 1% / 0.1% low, longest frame, hitches and
  how long the game's threads wait for a processor; the report is saved on the desktop. Nothing in the game is opened
  (kernel trace and DXGI frames only).
- **Thread map** (1.17.0, Scheduler section): the game's processes and threads as a live graph (who wakes whom, who
  waits on whose lock, who takes whose core, who calls whom) over a 30 s history. Its kernel trace runs only while the
  page is on screen.
- **Cache cleaner**, autostart through Task Scheduler, a one-click report, a colour-coded log. The program measures its
  own load by thread; when threads outside the known list use a quarter of a core or more, the log names them.

No kernel driver, no DLL injection, no hooks, no memory patching — only documented Windows APIs
(affinity, CPU Sets, ideal processor, priority classes, thread priority and boost, I/O and memory
priority, EcoQoS). Nothing leaves the machine.

## Requirements

- Windows 10 (1903 or newer) or Windows 11, x64
- Administrator rights (the program asks for them at start)
- No runtime to install: the release build is one static exe (about 8 MB)

## Quick start

1. Run `DomainScheduler.exe`.
2. Make sure your game is on the watchlist (`cs2.exe` is there by default; add others on the
   **Processes** or **Settings** page).
3. Press **Turn on** and start the game. Keep it in the foreground for a few seconds.
4. To undo everything: **Turn off**, **Revert**, or exit from the tray icon.

## Recommended settings by CPU

The defaults are meant to be safe. Auto picks the mask by itself; the table shows what it picks
and what is worth trying by hand.

| CPU | What Auto does | Worth trying | Expectation |
|---|---|---|---|
| **Dual-CCD X3D** — 7900X3D, 7950X3D, 9900X3D, 9950X3D | Game on the big-cache CCD (`vcache`), background on the other CCD | Core reservation of the non-game CCD (Masks page, needs a reboot) — only after you have measured the default | The CPU family with the most to gain |
| **Intel hybrid** — 12th–14th gen, Core Ultra | Game on P-cores (`pcores`), background on E-cores | `nosmt` (one thread per P-core) — test both | Usually noticeable on lows when background load is real |
| **Single-CCD X3D** — 5800X3D, 7800X3D, 9800X3D | Nothing to split: mask `auto`, threads spread over physical cores, background lowered | `nocore0` | Small; mostly from the background layer |
| **Ryzen with two equal CCDs** — 5900X, 5950X, 7900X, 7950X, 9900X, 9950X | No automatic split (the caches are equal) | Mask `ccd0` or `ccd1` by hand to keep the game inside one CCD — test both | Depends on the game; can go either way |
| **Ordinary single-domain CPUs** — Ryzen 5600 / 5700X / 7600 / 7700, Intel up to 11th gen | Mask `auto`, priorities, background lowered | `nocore0` on 8+ cores | Small; do not expect much |
| **Fewer than 6 cores** | Priorities and background only; the mask is never cut below 4 logical processors | Leave the defaults | Often none: there is no spare core to move work to |
| **More than 64 logical processors or several processor groups** | Works in group 0 only; no split, no reservation | — | Not a target platform |

Settings that apply to every CPU:

| Setting | Recommendation |
|---|---|
| Auto | **On** — this is the main mode |
| Built-in system presets, background layer (`BgOn`), warm-up | **On** (default) |
| Thread CPU Sets (hard limit of a thread to one core) | **Off**. Free migration is usually better |
| SMT / Hyper-Threading | Leave **enabled** in the BIOS; use `nosmt` in the mask if you want to test without it |
| Feature flags | Device policy write and GPU overclocking **open** since 1.16.0 (both ask before writing); timer resolution, live DPC/ISR flag and soft close **closed** unless you know why you need one |
| Heat policy in presets | `none` |

## How to check the effect

1. Pick a repeatable scene (the same map, the same demo or benchmark run).
2. Record 1% low, 0.1% low and p99 frame time with CapFrameX or PresentMon — three runs with the
   program off, three with it on.
3. Compare the *typical* values. A difference of a few percent is noise.
4. If the lows did not improve on your system, the program has nothing to give you there — turn it
   off; nothing stays changed.

## Honest limits

- The ideal processor is a hint: Windows may ignore it.
- Thread recognition is a heuristic; games without thread names fall back to the mask and priorities.
- On dual-CCD X3D the big-cache CCD is the better choice for most games, not for every game.
- If the GPU is the bottleneck, CPU scheduling has little to change.
- While an anti-cheat client that protects the game is running (for example FACEIT with CS2), the
  game itself is not touched at all; only the background layer works. Games under kernel
  anti-cheats (Vanguard, EasyAntiCheat, BattlEye and others) are never opened. Check the rules of
  your league or platform before using any third-party tool.
- The built-in presets are a starting point; review them for your own software.
- The program runs with administrator rights (without a UAC prompt only if that playbook option was ticked; off by
  default since playbook 2.1.9). Everything it
  restores or acts on by itself is kept where only administrators can write (see below), and every restored value is
  checked against what the program itself writes. Since 1.17.0 the program is one static exe with no runtime host:
  nothing is unpacked into `%TEMP%` at start (the C# Toolbox 1.16.x and older had its .NET host unpack native
  libraries there on the first start of a run). With UAC prompts switched off, any program of an administrator account
  can get administrator rights anyway; these protections matter most when UAC prompts are on.

## GPU overclock

Behind its own feature flag (Settings → feature flags → GPU overclocking, open by default since 1.16.0; nothing is written until you apply a profile) the Graphics card page
offers a small, careful overclock with named profiles, one of which can be applied at every start.

- **NVIDIA** (through the NVIDIA driver): core and memory clock offsets and the voltage boost (0–100 % of what the
  maker allows).
- **AMD Radeon** (through the AMD driver's own interface, `atiadlxx.dll`, installed with every Radeon driver):
  - RX 5000/6000/7000/9000 (Overdrive8): the maximum GPU clock, the maximum memory clock, the power limit (percent)
    and, where the driver offers it, a voltage offset — lowering only (undervolt), never raising;
  - Polaris RX 470/480/570/580/590 and Vega 56/64 (OverdriveN): the clock of the top performance state of the GPU and
    of the memory, and the power limit. The lower states and every voltage are written back exactly as the driver
    reports them.
  - Only active adapters of a physical card are used (one per card; integrated Radeon graphics are left out).
- Clocks are stored as **offsets from the card's stock values** for both makers (on AMD: from the stock maximum clock
  the driver reports, read again at every apply), so a profile never holds an absolute clock. Every value is cut to
  the range the driver reports, to its step and to the program's own caps (+300 MHz core, +1500 MHz memory, never
  below −300 MHz), and is read back after writing.
- **Reset to stock** writes the driver's own stock values (AMD Overdrive8: the driver's reset flag with its defaults;
  OverdriveN: the driver's stock state table, power limit back to automatic). It reads everything fresh, so it also
  works after a failed apply or in a new session.
- **Apply at start** re-applies the marked profile only on a card of the maker it was saved for, and never after
  Windows went down unexpectedly since the last apply (blue screen, freeze with a power-off): the crash guard is the
  same for both makers. Values that crashed Windows are remembered (since 1.16.0 Test 1): marking the same values or
  higher for the start again asks first, with "No" as the default, and the profile shows the date of the crash.
- No fan control, no memory timings, no process is closed. If the driver or its overclocking interface is missing,
  the page says so and nothing is written. AMD support is new in 1.15.0 Test 1.5 and has not yet been run on real
  Radeon cards: please report your card, driver version and what the page showed.

## Project status

Version **1.17.1** (delivered through Toolbox's own update). New since 1.17.0:

- **Ideal cores for all game threads**: Dynamic Top-5 now also looks after every other thread of the game (DLL threads
  too) on the same one-second tick: each gets a preferred core away from core 0, the Top-5 cores and the cores drivers
  keep busy, and is moved only for a clear reason. Switch in Settings and on the Game threads page (on), with a short
  line of what it did and its last moves. On the developer's 7800X3D (Top-5 on 2, 4, 6, 8, 10, graphics driver on
  12-15) the other threads use 3, 5, 7, 9 and 11. The tick costs about 7 µs for 150 threads and allocates nothing.

Version **1.17.0** (shipped with playbook 2.3.2, built and attested by GitHub, in [Releases](https://github.com/6kane1/DomainOS/releases)). New since 1.16.1:

- **Native program**: the same Toolbox rebuilt as a Rust core and a C++20 window (Dear ImGui, DirectX 11, Win32), one
  static exe of about 8 MB instead of the self-contained .NET 8 / WPF exe of about 71 MB. Same exe name, install folder,
  scheduled task, settings, state folder, command-line switches and update keys. The window redraws only on input, on a
  timer a page asked for or when the program's state changed.
- **Look**: a static dark background with a soft pink and violet glow, translucent panels, soft buttons.
- **Overview page**: the row "Boost / Reapply / Resync / Revert" is gone. Auto boosts and re-applies by itself, and
  Revert did the same as "Undo everything" in the status panel. "Resync" is now **Reload hardware and presets** in the
  advanced part of Settings.
- **Thread map** page (Scheduler section), see [What it does](#what-it-does).
- From the 1.16.2 test builds: the burst guard never limits the graphics driver's own processes (NVIDIA and AMD containers, panels, overlays) or DomainSchedTest, so measurements stay exact. **CS2 under FACEIT** is tuned too (switch in Settings, on): FACEIT confirmed in writing on 2026-11-25 that its anti-cheat allows it; only CS2 and only FACEIT, every other anti-cheat stays untouched. The core mask is used only on processors with more than 8 cores of two kinds (Intel with E-cores, Ryzen with two CCDs). The Black box is **off by default** now (switch in Settings). The System changes page keeps only background recording, pointer acceleration and the two Windows Update changes (the playbook still sets the privacy, advertising and interface ones).

Version **1.16.1** (shipped with playbook 2.3.1). New since 1.16.0:

- **Wait probe** (Settings, on): while Auto handles a game, the kernel trace adds context switches, ready threads and hard page faults for 250 ms every 2 s. Dynamic Top-5 ranks the game's threads by load **and** by their p99 wait for a core (a thread that waits 1 ms counts like a 30% share), and a slot leaves its processor for one of three separate reasons: other programs keep it busy (without the interrupt time), drivers take more than 5% of it in DPC/ISR, or its own thread keeps waiting (p99 above 0.5 ms).
- **Core mask**: processors with one cache domain and no E-cores give the game every core at any core count (6 or fewer before). On the developer's 7800X3D the mask made the main game threads wait for a core 13 times per 30 s in CS2 deathmatch, the longest wait 940 µs (202 µs without), with 1% / 0.1% low equal. A **Core mask** button on the game threads page switches it off or on in one click (Auto picks it up within ~10 s), with a short guide on testing it.
- **Graphics-driver and interrupt cores held 60 s**: a driver core stays marked until the driver was quiet on it for 60 s, a busy interrupt core is let go after 60 s of calm (taken after 20 s), so the game's threads are no longer moved every few seconds.
- **Burst guard and memory**: the game's memory is held only when the game reads more than 30 of its own pages per second back from disk, and the hold is lifted after 10 s if it did not halve them.
- **Black box** (page in the Scheduler section, switch in Settings, on): Windows' kernel trace and the game's frames stay in memory for about 30 s; a frame longer than 3x the median and two refreshes is explained in plain words (game or GPU, driver, another program, not enough free cores) with advice; frames of 100 ms and more are loading; Windows' own processes never get "close it" advice. Only reads, nothing on disk but small JSON reports (last 50).
- Update feeds also come as one file with the signature inside (`*.signed`); the language switch no longer fails on the status panel.

Measured on the developer's Ryzen 7 7800X3D + RTX 5060 (CS2 deathmatch, 3 runs of 30 s, mask off): 1% low 429, 0.1% low 230, the longest wait of a game thread for a core 183 µs, no wait of a main game thread over 0.5 ms.

Previous version **1.16.0** (shipped with playbook 2.3.0). New since 1.15.0:

- **Updates and testing**: one page for the update check, rolling back to the previous version, the tester key and the update channel (they were in About and Settings).
- **Dynamic Top-5 is the only thread mode**: the role plan is removed, in the developer's measurements it only made things worse.
- **Devices**: instead of the big table, one line and an "Apply the layout" button; its confirmation lists which device gets which core. When the layout is already in place, there is nothing to apply.
- **Burst guard** (Settings, on, soft mode): while a game is handled, a background program of your session that keeps using 0.3 of a processor or more for 6 s gets the smallest scheduling weight of a Job Object of its own: it is never stopped, it only gives way when the processors are busy (soft mode off: a hard cap of half a processor). Child processes break away, so a game started from it is never limited. Lifted when you switch to that program, when the game ends, Auto stops or Toolbox exits, and after a crash at the next start. Services, launchers, the shell, audio / voice / streaming / VR, network boosters, security software and anti-cheats are never touched. Only when free memory is low (under 10% of RAM or 1.5 GB) the game keeps its current memory as a hard minimum (at most 25% of RAM). On a clean PC it does nothing; after a game the log says what was limited.
- **6 cores or fewer** (one cache domain, no E-cores): Auto gives the game every core and the device layout gives no device a core of its own. On a tester's i5-10400F the old mask cost 6% of the 1% low in CS2. 8 cores and more work as in 1.15.0. Setting `AutoMask` auto | on | off (`--auto-mask`), `--auto on|off|status` for scripted measurements.
- Check page (one measured run), crash memory of the GPU overclock, own load per thread, shorter texts in Settings.

Test builds are encrypted and live in a private repository that only the developer and invited testers can see. 401 automated tests run against a simulated Windows (`cargo test`, see [Tests](#tests)), including fuzz tests of every parser of outside data and a key-rotation drill. Hardware coverage is narrow: please report your CPU, Windows build and before/after numbers in the issues.

## Build from source

The source of the native Toolbox (1.17.0 and newer). It is published in `toolbox/` of
[6kane1/DomainOS](https://github.com/6kane1/DomainOS) and at the root of
[6kane1/DomainScheduler](https://github.com/6kane1/DomainScheduler); the C# source of 1.16.x and older stays on branch
`csharp` of 6kane1/DomainScheduler.

### Layout

```
core/                Rust crate dos_core: all the logic; a static library for the exe (libdos_core.a), an rlib for the tests
  src/base           shared types, settings, translations, log, feature flags, the state folder, the version
  src/native         Windows calls behind a trait, and the fake Windows the tests run against
  src/topo           processor topology, core masks, core split, interrupt layout, core reservation
  src/engine         priorities, background layer, guard (denylist, anti-cheats), session, built-in tables
  src/auto           Auto, Dynamic Top-5, burst guard, tuner, HUD, the Check page's statistics
  src/etw            kernel traces: DPC/ISR, frames, wait probe, Black box, Check trace, Thread map trace
  src/system         store, updater, autostart, boot guard, legacy import, system changes, devices, maintenance
  src/gpu            GPU overclock (NVIDIA, AMD) and its profiles
  src/ffi            the C ABI for the app (header app/src/dos_core.h): the long-lived objects, start-up, command line,
                     single instance, update hand-over
  res/               embedded into the exe: lang/ (en.json, ru.json, tables.json, threads.ini), ru.native.json
  tests/             integration tests (parity_gaps.rs), signature test samples (data/sigsample)
threadmap/           Rust crate threadmap: the Thread map logic (graph, history, layout), linked into dos_core (threadmap.h)
app/                 the window: C++20, Dear ImGui + DirectX 11 + Win32; no logic, everything that decides or changes
                     something is a call into the core
  src/               shell (main, window chrome, sidebar, theme, widgets, tray, error popup, charts, bridge to the core)
  src/pages/         one file per page
  src/app.rc         icon, manifest, embedded fonts
  third_party/imgui  Dear ImGui 1.92.9b (vendored, with its LICENSE.txt)
  res/               app.ico, app.manifest (administrator, per-monitor DPI v2), Fonts/ (Inter, JetBrains Mono)
tools/sync-lang.sh   refreshes core/res/lang from the C# source
build.sh             the whole build
LICENSE
```

### Toolchain and build

The Windows exe is cross-built on Linux:

- Rust **1.100.0** with the target `x86_64-pc-windows-gnullvm`;
- **llvm-mingw 20260922** (`llvm-mingw-20260922-ucrt-ubuntu-22.04-x86_64`): `x86_64-w64-mingw32-clang++`, `windres`,
  `llvm-strip`.

`./build.sh` runs, in this order: `cargo test` (core with `--features testsupport`, threadmap), `cargo build --locked
--release --target x86_64-pc-windows-gnullvm` (the static library `libdos_core.a`), then `x86_64-w64-mingw32-clang++`
compiles Dear ImGui and `app/src`, `windres` compiles `app/src/app.rc`, everything is linked into one static exe and
`llvm-strip` strips it. The result is `out/DomainScheduler.exe` (about 8 MB).

| Option | What it does |
|---|---|
| `./build.sh --no-test` | skips `cargo test` |
| `OUT=path/DomainScheduler.exe` | where the exe is written (default `out/DomainScheduler.exe`) |
| `OBJ=folder` | the folder for the C++ object files (default `obj`, deleted after the build) |
| `CARGO_TARGET_DIR=folder` | Cargo's own build folder (with `OBJ`: several builds side by side) |
| `MINGW=path` | the `bin` folder of llvm-mingw (default `~/sdk/llvm-mingw-20260922-ucrt-ubuntu-22.04-x86_64/bin`) |
| `NO_CORE=1` | the app alone on `app/src/core_stub.cpp`, no Rust build (for work on the window only) |

`RUSTUP_HOME` and `CARGO_HOME` default to `~/sdk/rust/rustup` and `~/sdk/rust/cargo`.

**The version has one source:** `version` in `core/Cargo.toml`. The program reads it at build time
(`env!("CARGO_PKG_VERSION")`), and the build writes it into the exe's file properties (VERSIONINFO).

**Shared resource files:** `core/res/lang/{en.json,ru.json,tables.json,threads.ini}` are the same translations, built-in
tables and thread names as in the C# Toolbox, vendored here; `tools/sync-lang.sh` refreshes them from the C# source.
Texts that only the native Toolbox has are translated in `core/res/ru.native.json`, which is merged over `ru.json` at
start.

Third-party code: the Rust crates serde, serde_json, sha2, p256, aes-gcm, base64, indexmap and windows-sys (exact
versions in `Cargo.lock`), Dear ImGui 1.92.9b (MIT), the Inter and JetBrains Mono fonts (SIL Open Font License 1.1,
embedded into the exe).

### Tests

```
cd core && cargo test --locked --features testsupport   # 344 unit tests + 4 integration tests (tests/parity_gaps.rs)
cd threadmap && cargo test --locked                     # 28 tests
```

401 tests on Linux. A Windows build of the tests runs 364 core tests (6 of them only exist on Windows). The logic runs
against a fake Windows, so nothing on the PC is changed. On GitHub, `tests.yml` runs these tests on Ubuntu (area G of
`tests/run-all.py`) and on windows-latest.

### Proof that an exe is built from this source

`.github/workflows/toolbox-build.yml` of 6kane1/DomainOS builds Toolbox on GitHub's machines (ubuntu-latest): the
pinned toolchain (`.github/actions/native-toolchain`, llvm-mingw checked by SHA-256), the tests first, the crates exactly
as in `Cargo.lock` (`--locked`), build paths taken out of the exe (`--remap-path-prefix`). Every exe gets a signed
build-provenance attestation and an attested SPDX SBOM (`tools/make-sbom-native.py`: the crates, Dear ImGui, the fonts,
the build tools). Check any copy with the GitHub CLI:

```
gh attestation verify DomainOS-Toolbox-1.17.1.exe -R 6kane1/DomainOS
```

## Compatibility with the C# Toolbox

The native Toolbox replaces the C# one (WPF, .NET 8, up to 1.16.x) and keeps everything other parts of Domain OS rely
on: the exe name `DomainScheduler.exe`, the install folder `C:\Program Files\Domain OS`, the scheduled task
`DomainScheduler`, the settings in `HKCU\Software\DomainScheduler` and `HKLM\SOFTWARE\Domain OS\Toolbox`, the state
folder `%ProgramData%\Domain OS Toolbox` with the same files, the same command-line switches (`--auto on|off|status`,
`--auto-mask`, `--after-update` and the others) and the same update signature keys. The playbook and Undo work with both.

**Update feeds:** the native Toolbox reads `update/stable-native.json` and the sealed test feed `update/test-native.bin`.
The C# Toolbox 1.16.x keeps reading `update/stable.json` and `update/test.bin`; `stable.json` keeps the 1.16.1 entry.
A C# Toolbox is therefore **not** updated to the native one by itself: it comes with playbook 2.3.2, installed with AME
Wizard over Domain OS. The C# Toolbox shows a card when playbook 2.3.2 is out and can download it.

## Where things are stored

- **State folder** `%ProgramData%\Domain OS Toolbox` — presets, profiles, GPU profiles, the backups of device,
  registry, power and core-reservation changes, the crash-safe session file, the device boot guard, the log and update
  downloads (`Updates`); `native` holds the native libraries the C# Toolbox 1.16.x and older unpacked (the native
  Toolbox has none to unpack). Only SYSTEM and Administrators can change anything in it; other accounts can read it. Its owner and permissions are checked at every start: a folder
  someone else could have written to is renamed `…untrusted-<time>`, never read, and a fresh one is made.
- **Settings that make the program act by itself** (feature flags, the GPU overclock at start, one-shot clean-ups):
  `HKLM\SOFTWARE\Domain OS\Toolbox` (administrators only).
- **Window preferences** (language, position, charts, watchlist, mask choices): `HKCU\Software\DomainScheduler`.
- Autostart: Task Scheduler task `DomainScheduler`.
- **Settings → Factory reset** removes all of the above.

Versions before 1.15.0 Test 1.3 kept their data in `%LOCALAPPDATA%\DomainScheduler`, which any program of your
account can change. At start the program takes those files over once — only entries it could have written itself (a
planted entry is dropped and named in the log) — and renames the old files `*.migrated`. The feature flags and the
one-shot markers are copied from HKCU to HKLM once the same way. A GPU profile that was marked "apply at start" is
not applied from the old file as it is: at the first start the program asks once, in its own window and with the
profile's values shown, whether to keep applying it at start (Yes keeps the mark, No drops it; the profile stays).

Every backup is checked again when it is restored: only the keys, value names, types and ranges the program writes
are put back; anything else is refused and logged. Child processes (PowerShell, schtasks, bcdedit, wevtutil…) are
started from System32 by their full path; PowerShell runs without a profile and sees only Windows' own modules, and
no value is ever put into a command line as text.

## Update keys and key rotation

Update feeds (`update/stable-native.json`, the sealed test feed `update/test-native.bin`; the C# Toolbox 1.16.x reads
`update/stable.json` and `update/test.bin`) are signed with ECDSA P-256; the `.sig` next to each is the base64 DER
signature over SHA-256, made by `DomainOS/tools/sign-feed.sh`. Toolbox trusts a short list of public keys
(`Updater::trusted_keys()` in `core/src/system/updater.rs`): a feed is accepted when its signature checks with **any**
of them. The `.sig` format has not changed and carries no key id, so feeds signed with the main key keep working for every Toolbox
version in the field; the new code simply tries each trusted key.

| Key | Id (SHA-256 of the public key, first 16 hex) | Where the private half lives |
|---|---|---|
| main | `824da223f01c0905` | the author's PC (plus an offline backup copy) |
| backup | `a807a74083942dde` | offline only (USB stick / paper) |

`Updater::REVOKED_KEY_IDS` lists keys that must never be trusted again. It is checked against the fingerprint computed
from the key itself, so a revoked key is refused even if it is still listed as trusted.

**If the main key leaks or is lost:**

1. Stop signing with it. Make a new key pair: `openssl ecparam -name prime256v1 -genkey -noout | openssl pkcs8 -topk8
   -nocrypt -out NEW.pem` (`chmod 600 NEW.pem`); its public key: `openssl pkey -in NEW.pem -pubout -outform DER | base64 -w0`.
2. In `core/src/system/updater.rs` the old backup becomes `PUBLIC_KEY` (main), the new key becomes
   `BACKUP_PUBLIC_KEY`, the ids in `trusted_keys()` are updated (the test `built_in_key_list_is_consistent` checks
   them), and a leaked key's id goes into `REVOKED_KEY_IDS`. Build and test that Toolbox. The C# Toolbox 1.16.x still
   reads `update/stable.json` and trusts the same two keys: make the same change in its `Updater.cs` (branch `csharp`
   of 6kane1/DomainScheduler) and ship that build in `stable.json`.
3. Sign the feeds that ship them with the backup key:
   `DOMAINOS_SIGNING_KEY_BACKUP=/path/on/usb.pem sign-feed.sh --backup update/stable-native.json` (also
   `test-native.bin`, `stable.json`, `test.bin`). Installed versions already trust the backup key, so they accept the
   feed and update to the version that no longer trusts the compromised key.
4. The old backup is now the main signing key; keep `NEW.pem` offline as the next backup; delete every copy of the
   compromised key.

If only the backup key is lost or leaked, ship a Toolbox with a new `BACKUP_PUBLIC_KEY` (and the old backup id in
`REVOKED_KEY_IDS` if it leaked), signed with the main key as usual.

**Telling users (the same day a key leak is known):** a revocation reaches a PC only with the Toolbox update that
carries it, so until then a leaked key could sign a fake feed. To shorten that window:

1. Pin a short notice in Discord **02・news** and **08・changelog**, open a GitHub security advisory in
   6kane1/DomainOS and put a banner on the website: which key id leaked, since when, and what to do.
2. What users do: update Toolbox at once (Update on the home page), or, until the fixed version is out, turn off
   *Check for updates* in Settings and install nothing offered by Toolbox; the website and GitHub Releases (verify
   with `gh attestation verify`) stay the trusted source.
3. Publish the fixed Toolbox (signed with the backup key) as soon as it is built, and say so in the same places.
4. Afterwards: in the changelog, name the revoked key id and the new key ids, and what happened.

## Team

- **6kane1** — author and developer of Domain OS.
- **Zero Unflesh (zero_wake)** — co-owner of Domain OS and owner of AerialOS. Made a very large contribution to this project.

## License
**Domain OS License 2.0** — all rights reserved (see [LICENSE](LICENSE)). You may read the code and use the official releases free of charge, for personal non-commercial use, on your own PCs. Copying, re-uploading, publishing changed versions or builds based on this code, removing the author notices and selling are not allowed without the author's written permission. **"Domain OS", "Domain OS Toolbox" and the Domain OS logo identify the author's work: they may not be used for other playbooks, builds, tools, sites, servers or channels, and no other work may be presented as Domain OS or based on it.** Versions published before 18 November 2026 under MIT stay MIT for the copies obtained then.
Windows is a trademark of Microsoft; this project ships no part of Windows.
