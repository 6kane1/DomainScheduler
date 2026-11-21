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
- The program runs with administrator rights (without a UAC prompt only if that playbook option was ticked; off by
  default since playbook 2.1.9). Everything it
  restores or acts on by itself is kept where only administrators can write (see below), and every restored value is
  checked against what the program itself writes. Two things remain outside its control: the .NET host first unpacks
  the program's native libraries into `%TEMP%` on the very first start of a run (the program then starts itself again
  with them unpacked into its admin-only folder before any of them is loaded; if that is impossible it carries on with
  the `%TEMP%` copy and logs it), and environment variables of your account that make .NET load extra code (a
  profiler) still apply to that first start. With UAC prompts switched off, any program of an administrator account
  can get administrator rights anyway; these protections matter most when UAC prompts are on.

## GPU overclock

Behind its own feature flag (Settings → feature flags → GPU overclocking, closed by default) the Graphics card page
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
  same for both makers.
- No fan control, no memory timings, no process is closed. If the driver or its overclocking interface is missing,
  the page says so and nothing is written. AMD support is new in 1.15.0 Test 1.5 and has not yet been run on real
  Radeon cards: please report your card, driver version and what the page showed.

## Project status

Released version: **1.15.0** (shipped with playbook 2.2.0, built and attested by GitHub, in [Releases](https://github.com/6kane1/DomainOS/releases)): signed self-update with a backup key, AMD Radeon overclocking, a notice when a new playbook is out. 227 automated tests run against a
simulated Windows (`dotnet test tests/DomainScheduler.Tests`). Hardware coverage is narrow: please report
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

- **State folder** `%ProgramData%\Domain OS Toolbox` — presets, profiles, GPU profiles, the backups of device,
  registry, power and core-reservation changes, the crash-safe session file, the device boot guard, the log, update
  downloads (`Updates`) and the program's unpacked native libraries (`native`). Only SYSTEM and Administrators can
  change anything in it; other accounts can read it. Its owner and permissions are checked at every start: a folder
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

Update feeds (`update/stable.json`, the sealed test feed `update/test.bin`) are signed with ECDSA P-256; the `.sig`
next to each is the base64 DER signature over SHA-256, made by `DomainOS/tools/sign-feed.sh`. Toolbox trusts a short
list of public keys (`Updater.TrustedKeys`): a feed is accepted when its signature checks with **any** of them. The
`.sig` format has not changed and carries no key id, so feeds signed with the main key keep working for every Toolbox
version in the field; the new code simply tries each trusted key.

| Key | Id (SHA-256 of the public key, first 16 hex) | Where the private half lives |
|---|---|---|
| main | `824da223f01c0905` | the author's PC (plus an offline backup copy) |
| backup | `a807a74083942dde` | offline only (USB stick / paper) |

`Updater.RevokedKeyIds` lists keys that must never be trusted again. It is checked against the fingerprint computed
from the key itself, so a revoked key is refused even if it is still listed as trusted.

**If the main key leaks or is lost:**

1. Stop signing with it. Make a new key pair: `openssl ecparam -name prime256v1 -genkey -noout | openssl pkcs8 -topk8
   -nocrypt -out NEW.pem` (`chmod 600 NEW.pem`); its public key: `openssl pkey -in NEW.pem -pubout -outform DER | base64 -w0`.
2. In `Updater.cs` the old backup becomes `PublicKey` (main), the new key becomes `BackupPublicKey`, the ids in
   `TrustedKeys` are updated (the test `BuiltInKeyListIsConsistent` checks them), and a leaked key's id goes into
   `RevokedKeyIds`. Build and test that Toolbox.
3. Sign the feed that ships it with the backup key:
   `DOMAINOS_SIGNING_KEY_BACKUP=/path/on/usb.pem sign-feed.sh --backup update/stable.json` (also `test.bin`).
   Installed versions already trust the backup key, so they accept the feed and update to the version that no longer
   trusts the compromised key.
4. The old backup is now the main signing key; keep `NEW.pem` offline as the next backup; delete every copy of the
   compromised key.

If only the backup key is lost or leaked, ship a Toolbox with a new `BackupPublicKey` (and the old backup id in
`RevokedKeyIds` if it leaked), signed with the main key as usual.

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

- **6kane1** — author and owner of Domain OS.
- **Zero Unflesh (zero_wake)** — co-owner of Domain OS and owner of AerialOS. Made a very large contribution to this project.

## License
**Domain OS License** — all rights reserved (see [LICENSE](LICENSE)). You may read the code and use the official releases free of charge on your own PCs. Copying, re-uploading, publishing changed versions or builds based on this code, removing the author notices and selling are not allowed without the author's written permission. Versions published before 18 November 2026 under MIT stay MIT for the copies obtained then.
Windows is a trademark of Microsoft; this project ships no part of Windows.
