# DomainScheduler

**English** | [Русский](README.ru.md)

A user-mode Windows utility that tunes how the scheduler places a game's threads and how much
room background programs take. Its goal is **steadier frame times** — better 1% and 0.1% lows
and a more even frame pacing — not a higher average FPS.

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
- **Two thread modes.** *Role plan* (default): main / render / audio / worker threads are
  recognized and placed once. *Dynamic Top-5* (experimental): the five busiest threads are
  re-ranked every second and moved between cores and priorities as the load changes.
- **Watchlist and denylist.** Only programs you mark are tuned. System-critical processes and
  anti-cheat software are never opened or changed.
- **Background layer.** While a watched game runs, other programs get Below-normal priority,
  lowest I/O priority, low memory priority, EcoQoS and (where it makes sense) the non-game cores.
- **Built-in presets** for Windows components and common background apps; your own presets with
  import, export and profiles.
- **Core masks** with a visual CPU map, interrupt-aware refinement and optional core reservation.
- **Devices page**: read-only view of interrupt pinning (writing is an opt-in advanced feature).
- **Frame-time watchdog.** If p99 frame time gets worse after the changes, they are taken off,
  re-measured, and kept off if the regression is real.
- **Cache cleaner**, autostart through Task Scheduler, a one-click report, a colour-coded log.

No kernel driver, no DLL injection, no hooks, no memory patching — only documented Windows APIs
(affinity, CPU Sets, ideal processor, priority classes, thread priority and boost, I/O and memory
priority, EcoQoS). Nothing leaves the machine.

## Requirements

- Windows 10 (1903 or newer) or Windows 11, x64
- Administrator rights (the program asks for them at start)
- No runtime to install: the release build is self-contained

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
| Thread mode | **Role plan** (default). Try *Dynamic Top-5* for games without thread names or with a changing load, and compare |
| Thread CPU Sets (hard limit of a thread to one core) | **Off**. Free migration is usually better |
| SMT / Hyper-Threading | Leave **enabled** in the BIOS; use `nosmt` in the mask if you want to test without it |
| Feature flags (timer resolution, live DPC/ISR, device policy write, soft close) | **Closed** unless you know why you need one |
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

## Project status

Version 1.3.0. The engine, masks, Auto, presets and the roadmap features are covered by 90
automated tests that run against a simulated Windows. Hardware coverage is narrow: please report
your CPU, Windows build and before/after numbers in the issues.

## Build from source

```
dotnet publish src/DomainScheduler/DomainScheduler.csproj -c Release -r win-x64 --self-contained true -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true
dotnet test tests/DomainScheduler.Tests
```

.NET 8 SDK. The solution has three projects: `DomainScheduler.Lib` (logic and Windows calls),
`DomainScheduler` (WPF window, code-behind only) and `DomainScheduler.Tests` (xUnit).

Third-party packages: Microsoft.Diagnostics.Tracing.TraceEvent, System.Management.

## Where things are stored

- Settings: `HKCU\Software\DomainScheduler`
- Presets, profiles, log, session file: `%LOCALAPPDATA%\DomainScheduler`
- Autostart: Task Scheduler task `DomainScheduler`
- **Settings → Factory reset** removes all of the above.

## License
**Domain OS License** — all rights reserved (see [LICENSE](LICENSE)). You may read the code and use the official releases free of charge on your own PCs. Copying, re-uploading, publishing changed versions or builds based on this code, removing the author notices and selling are not allowed without the author's written permission. Versions published before 18 November 2026 under MIT stay MIT for the copies obtained then.
Windows is a trademark of Microsoft; this project ships no part of Windows.
