<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="banner-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="banner-light.png">
    <img alt="Custom YT" src="banner-light.png" width="480">
  </picture>
</p>

<h1 align="center">Custom YT</h1>

<p align="center">
  <img alt="Linux" src="https://img.shields.io/badge/Linux-FCC624?style=for-the-badge&logo=linux&logoColor=black">
  <img alt="Windows" src="https://img.shields.io/badge/Windows-0078D6?style=for-the-badge&logo=windows&logoColor=white">
  <img alt="macOS" src="https://img.shields.io/badge/macOS-000000?style=for-the-badge&logo=apple&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white">
</p>

**A terminal-native YouTube client that plays video without loading a browser.**

Chrome and Firefox weren't built for watching a video — they were built for running a full application platform, and YouTube's web player rides on top of that overhead. Custom YT skips it entirely: it pulls the stream directly with `yt-dlp` and plays it through `mpv`'s hardware-accelerated pipeline. No DOM, no JS engine, no tab process. Just the video.

Built for old laptops, low-RAM Linux boxes, and anyone who's tired of a fan spinning up just to watch a 10-minute video.

```
Chrome tab:    [==========================] 1.2–2.5 GB RAM, fans on
Custom YT:     [==]                          65–95 MB RAM, silent
```

---

## Benchmarks

Two rounds of internal testing — first against raw browser playback, then a second pass optimizing the player pipeline itself (ffplay + software filters → mpv + hardware overlay). Both are included below so the numbers aren't cherry-picked.

### Round 1 — Browser playback vs. Custom YT

| Metric | Browser Playback | Custom YT | Difference |
|---|---|---|---|
| RAM Usage | 1.2 GB – 2.5 GB | < 95 MB | ~90% less RAM |
| CPU Load (720p30 + subtitles) | 95% – 100% (throttling) | 14% – 28% | ~70% less CPU |
| Frame Drops | 15–40 dropped/min | 0 | Eliminated |
| Startup Latency | ~4.5s | ~1.1s | 4x faster |

### Round 2 — Internal pipeline: ffplay+software filters vs. mpv+hardware overlay

| Metric | Old Approach (ffplay + software filters) | Updated Approach (mpv + hardware overlay) | Impact |
|---|---|---|---|
| RAM Consumption | 140 MB – 210 MB (uncapped demuxing buffers + software filter allocations) | 65 MB – 95 MB (hard-capped at 15MiB demuxer cache via `--demuxer-max-bytes`) | ~50% RAM savings — safe for 2GB systems |
| CPU Load (video only, 720p30) | 45% – 65% (software YUV conversion) | 12% – 25% (direct H.264/AVC hardware decoding) | ~60% lower CPU load |
| CPU Load (video + subtitles) | 95% – 100% (CPU throttling — single-threaded frame rasterization via `-vf subtitles`) | 14% – 28% (subtitles rendered on a transparent GPU/X11 overlay layer) | ~70% lower CPU load |
| Frame Drops (720p30) | 15–40 dropped/min with subtitles active | 0 | Stutter eliminated |

**Why this matters in practice:** on a 2GB RAM machine, a browser tab alone can consume more memory than the machine has to spare, before you've opened anything else. Custom YT's entire footprint fits comfortably alongside a code editor and a terminal — the exact setup this project was built to run on.

---

## How it works

Two mature, widely-used open-source tools, wired together with a minimal Rust layer that keeps memory and CPU usage predictable:

- **`yt-dlp`** extracts the raw video/audio stream URL — no browser needed to resolve it
- **`mpv`** plays that stream directly, using GPU hardware decoding and a hardware overlay for subtitles instead of burning them into software-rendered frames
- **Custom YT** (Rust, Tokio async runtime) glues the two together: search, pagination, quality selection, and clean process lifecycle management, with a hard-capped demuxer cache (`--demuxer-max-bytes=15MiB`) so memory use never runs away

The result is a pipeline where every layer between "you press play" and "video on screen" is deliberately minimal.

---

## Install

Custom YT runs on **Linux, macOS, and Windows**. The Rust binary itself is fully cross-platform — what differs per OS is how you install the two runtime dependencies it shells out to.

### 1. Build

Same command everywhere:

```bash
cargo build --release
```

### 2. Install runtime dependencies

Custom YT needs `mpv` and `yt-dlp` on your `PATH`. `ffmpeg` isn't strictly required but is strongly recommended — `yt-dlp` uses it to merge separate audio/video streams for higher-quality playback.

**Linux (Debian / antiX / Ubuntu)**
```bash
sudo apt update && sudo apt install --no-install-recommends -y mpv yt-dlp ffmpeg
```

**macOS (Homebrew)**
```bash
brew install mpv yt-dlp ffmpeg
```

**Windows (Scoop)**
```powershell
scoop install mpv yt-dlp ffmpeg
```
**Windows (Chocolatey)**
```powershell
choco install mpv yt-dlp ffmpeg
```

### 3. Run

**Linux / macOS**
```bash
./target/release/custom_yt
```

**Windows (PowerShell / Windows Terminal)**
```powershell
.\target\release\custom_yt.exe
```

`--no-install-recommends` (Linux) keeps the dependency footprint minimal — no extra packages you don't need, in keeping with the low-RAM design goal above.

### Platform notes

- **PATH**: `mpv` and `yt-dlp` must be resolvable on your system PATH. On Windows, either add mpv's install folder to PATH or place `mpv.exe` next to the Custom YT binary. If `yt-dlp` was installed as a Python package, make sure its scripts directory is on PATH too.
- **Playback window**: on Windows, mpv opens its own player window separate from the terminal — that's expected. Windows Terminal is just the host for the CLI; mpv still renders video the way it does on any OS.
- **Hardware decoding**: decode flags and hardware-acceleration behavior can vary by OS and GPU driver. If a quality preset stutters on your machine, try stepping down a preset — see [Usage](#usage) below.
- **WSL**: not required. Custom YT runs natively on Windows; WSL is only worth considering if you specifically want a Linux shell environment alongside it.

---

## Usage

1. Choose **Search** or **Playlist** mode
2. Browse results with `n` (next) / `p` (previous), or jump to an item by number
3. Pick a quality preset:
   - `1` — 360p (lightest, CPU-constrained machines)
   - `2` — 480p (balanced)
   - `3` — 720p30 (recommended — this is the profile benchmarked above)
   - `4` — 1080p (full quality, more capable hardware)
4. Playback starts in under two seconds — no page load, no ads, no tracking scripts

---

## Who this is for

- Anyone running Linux on hardware with 2–4GB of RAM, where a browser tab alone can eat the whole budget
- Developers on any OS who want YouTube open in the background without it competing with their IDE for memory
- Anyone who wants to watch a video without the browser tab it came from staying open

---

## License

Open source. See `LICENSE` for full terms.