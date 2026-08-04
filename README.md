# Custom YT

> A tiny, high-efficiency Rust YouTube CLI built for low-end Linux machines.

> Search, choose, and play YouTube from the terminal in seconds.

```text
	____            __           __        _______ __
  / __ \__  _______/ /____  ____/ /__     / ____(_) /___
 / / / / / / / ___/ __/ _ \/ __  / _ \   / /   / / / __ \
/ /_/ / /_/ / /  / /_/  __/ /_/ /  __/  / /___/ / / /_/ /
\___\_\\__,_/_/   \__/\___/\__,_/\___/   \____/_/_/\____/
```

## Why People Share It

Custom YT is the kind of tool that gets attention because it solves a familiar problem in a surprisingly small package.

- It feels fast the moment it launches.
- It works on machines people usually give up on.
- It replaces a heavyweight browser workflow with a single terminal flow.
- It has a memorable identity, not just a utility name.

## Viral-Ready Snapshot

If you want to describe the project in one line:

> A Rust YouTube CLI that makes low-end Linux machines feel useful again.

If you want a shorter social post:

> Built a tiny Rust YouTube CLI for low-RAM Linux boxes. Search, pick, and play without opening a browser.

Custom YT exists for a simple reason: sometimes you want to search and watch YouTube from a terminal without dragging in a heavy desktop app, browser session, or background service. This project keeps the workflow minimal, fast, and easy to audit so it can run comfortably on systems with very limited RAM and CPU headroom.

## What It Is For

- Searching YouTube directly from the terminal.
- Picking from the top 5 results without a browser.
- Playing H.264/AVC streams with `ffplay` to reduce decoding overhead.
- Staying lightweight enough for Debian, antiX, and other low-spec Linux setups.

## Why This Project Exists

Most modern video clients are built for convenience first and resource use second. Custom YT is intentionally the opposite. It aims to be:

- small enough to compile quickly,
- predictable enough to audit easily,
- and conservative enough to stay usable on older hardware.

## Features

- Terminal-first YouTube search and playback.
- Top 5 search result selection.
- Quality presets for 360p, 480p, 720p, and 1080p.
- AVC-first stream selection to avoid unnecessary VP9/AV1 decoding load.
- Buffered `ffplay` playback tuned for stability on weak connections.
- Minimal Rust dependency surface: `tokio` and `anyhow` only.

## Architecture

Custom YT is intentionally simple and linear:

```text
user input -> yt-dlp search -> result selection -> stream extraction -> ffplay playback
```

The code is structured around a small set of responsibilities:

- Input handling for query and menu selection.
- Discovery through `yt-dlp` search output.
- Format resolution with low-CPU H.264/AVC fallbacks.
- Playback orchestration with `ffplay` and conservative buffering.

## Design

The visual and interaction design is built around a few ideas:

- terminal-native output that stays readable on small screens,
- a branded startup banner to give the tool identity,
- a concise command manual so the workflow is obvious immediately,
- and low-noise output so the CLI feels fast rather than crowded.

## Requirements

### Runtime

- `yt-dlp`
- `ffplay` from `ffmpeg`

### Build

- `gcc`
- `pkg-config`

## Quick Install for Debian / antiX / Ubuntu

If required tools are missing, install the minimal package set with:

```bash
sudo apt update && sudo apt install --no-install-recommends -y ffmpeg yt-dlp build-essential
```

The `--no-install-recommends` flag matters here because it keeps the install lean and avoids pulling in desktop extras that are unnecessary on low-resource machines.

## Build

```bash
cargo build
```

## Run

```bash
cargo run
```

## How It Works

When launched, the program:

1. Prints a startup banner and command manual.
2. Checks for the required runtime binaries.
3. Prompts for a YouTube search query.
4. Fetches the top 5 results with `yt-dlp`.
5. Lets you select a result and a quality preset.
6. Resolves a direct stream URL.
7. Launches `ffplay` and waits for playback to finish.

## Command Flow

The app does not hide its behavior behind a large abstraction layer. It uses a straightforward pipeline:

```text
search query -> yt-dlp search -> choose result -> choose quality -> extract stream URL -> ffplay playback
```

## Open Source Notes

This project is designed to stay approachable for contributors and easy to fork for personal use cases. The code is intentionally small so that changes to search behavior, playback flags, or quality presets remain simple to reason about.

## Share Kit

Want to help the project spread?

- Star the repo if you like small terminal tools.
- Share the one-line description above with a screenshot or terminal clip.
- Mention the low-spec Linux angle, since that is the strongest hook.
- Fork it if you want to adapt the playback behavior or key bindings.

If you want to adapt it for your own setup, the most likely places to change are:

- `src/main.rs` for search and playback behavior,
- `Cargo.toml` for dependency policy,
- and this README for project-specific documentation.

## License

Add your preferred open-source license here if one has not been selected yet.
