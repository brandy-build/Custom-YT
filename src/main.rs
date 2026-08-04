use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tokio::signal;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Clone)]
struct SearchResult {
    title: String,
    id: String,
}

#[derive(Debug, Clone)]
struct PlaylistResult {
    title: String,
    url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    print_startup_banner();
    ensure_required_binaries()?;

    println!("Search mode:");
    println!("  [1] Video search");
    println!("  [2] Playlist search");
    let search_mode = prompt_choice_with_default("Select mode [1-2, default 1]: ", 1, 2, 1)?;

    let query = prompt_nonempty("Search YouTube: ")?;
    let selected = if search_mode == 1 {
        select_video_from_paginated_search(&query, 10).await?
    } else {
        let playlists = fetch_playlist_search_results(&query, 5).await?;
        if playlists.is_empty() {
            bail!(
                "No playlist results found. Try a different query or include 'playlist' in search text"
            );
        }

        println!();
        println!("Playlist results:");
        for (index, playlist) in playlists.iter().enumerate() {
            println!("  [{}] {}", index + 1, playlist.title);
        }

        let playlist_choice = prompt_choice("Select a playlist [1-5]: ", 1, playlists.len())?;
        let selected_playlist = playlists
            .get(playlist_choice - 1)
            .cloned()
            .ok_or_else(|| anyhow!("invalid playlist selection"))?;

        let playlist_videos = fetch_playlist_videos(&selected_playlist.url, 10).await?;
        if playlist_videos.is_empty() {
            bail!("Selected playlist has no playable videos");
        }

        println!();
        println!("Top 10 videos from selected playlist:");
        for (index, result) in playlist_videos.iter().enumerate() {
            println!("  [{}] {}", index + 1, result.title);
        }

        let video_choice_prompt = format!("Select a video [1-{}]: ", playlist_videos.len());
        let video_choice = prompt_choice(&video_choice_prompt, 1, playlist_videos.len())?;
        playlist_videos
            .get(video_choice - 1)
            .cloned()
            .ok_or_else(|| anyhow!("invalid video selection"))?
    };

    println!();
    println!("Quality presets:");
    println!("  [1] 360p  (Ultra Light - Low CPU)");
    println!("  [2] 480p  (Balanced - Fast)");
    println!("  [3] 720p  (HD - High Quality) [Default]");
    println!("  [4] 1080p (Full HD - High Performance)");

    let quality = prompt_choice_with_default("Select a quality [1-4, default 3]: ", 1, 4, 3)?;
    let format_filter = format_filter_for_quality(quality);
    let subtitle_mode = prompt_choice_with_default(
        "Subtitles [1=off, 2=English auto/manual, default 1]: ",
        1,
        2,
        1,
    )?;

    println!();
    println!("Resolving stream URL...");
    let watch_url = format!("https://www.youtube.com/watch?v={}", selected.id);
    let stream_url = resolve_stream_url(&watch_url, format_filter).await?;

    let subtitle_file = if subtitle_mode == 2 {
        println!("Fetching subtitles...");
        download_subtitle_file(&watch_url, &selected.id, "en").await?
    } else {
        None
    };

    println!("Launching playback via mpv...");
    let playback = launch_mpv(&stream_url, &selected.title, subtitle_file.as_deref()).await;

    if let Some(path) = subtitle_file {
        let _ = fs::remove_file(path);
    }

    playback?;

    Ok(())
}

fn ensure_required_binaries() -> Result<()> {
    // Requires mpv for zero-overhead hardware-accelerated subtitle overlays
    let required = ["yt-dlp", "mpv"];
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|binary| !binary_on_path(binary))
        .collect();

    if missing.is_empty() {
        return Ok(());
    }

    println!("Missing required system binaries: {}", missing.join(", "));
    println!(
        "Install them on Debian/antiX/Ubuntu with:\n\n  sudo apt update && sudo apt install --no-install-recommends -y mpv yt-dlp"
    );
    bail!("required system binaries are missing");
}

fn print_startup_banner() {
    let banner = include_str!("../ascii-art.txt").trim_matches('\n');
    let compact_banner = banner.lines().take(5).collect::<Vec<_>>().join("\n");
    println!("{compact_banner}");

    println!("Command Manual");
    println!("  1. Enter a search query.");
    println!("  2. Pick video mode or playlist mode.");
    println!("  3. In video mode, use n/p paging and pick from visible 10 results.");
    println!("  4. Choose a quality preset, or press Enter for 720p.");
    println!("  5. Optionally enable subtitles.");
    println!("  6. mpv plays the video with hardware-overlay subtitles.");
    println!();
    println!("Architecture");
    println!("  input -> yt-dlp search -> result selection -> stream extraction -> mpv");
    println!("Design");
    println!("  terminal-first, low-RAM, AVC-preferred, and hardware-overlay optimized");
    println!();
}

fn binary_on_path(name: &str) -> bool {
    let path_env = match std::env::var_os("PATH") {
        Some(value) => value,
        None => return false,
    };

    for entry in std::env::split_paths(&path_env) {
        let candidate = entry.join(name);
        if is_executable_file(&candidate) {
            return true;
        }

        #[cfg(windows)]
        {
            let candidate = entry.join(format!("{name}.exe"));
            if is_executable_file(&candidate) {
                return true;
            }
        }
    }

    false
}

fn is_executable_file(path: &Path) -> bool {
    match path.metadata() {
        Ok(metadata) => {
            if !metadata.is_file() {
                return false;
            }

            #[cfg(unix)]
            {
                return metadata.permissions().mode() & 0o111 != 0;
            }

            #[cfg(not(unix))]
            {
                return true;
            }
        }
        Err(_) => false,
    }
}

fn prompt_nonempty(prompt: &str) -> Result<String> {
    loop {
        print!("{prompt}");
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;

        let trimmed = input.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_owned());
        }

        println!("Please enter a value.");
    }
}

fn prompt_choice(prompt: &str, min: usize, max: usize) -> Result<usize> {
    loop {
        print!("{prompt}");
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;

        let trimmed = input.trim();
        match trimmed.parse::<usize>() {
            Ok(value) if (min..=max).contains(&value) => return Ok(value),
            _ => println!("Enter a number between {min} and {max}."),
        }
    }
}

fn prompt_choice_with_default(
    prompt: &str,
    min: usize,
    max: usize,
    default: usize,
) -> Result<usize> {
    loop {
        print!("{prompt}");
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;

        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(default);
        }

        match trimmed.parse::<usize>() {
            Ok(value) if (min..=max).contains(&value) => return Ok(value),
            _ => println!("Enter a number between {min} and {max}, or press Enter for the default."),
        }
    }
}

async fn fetch_video_search_results(query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let search_term = format!("ytsearch{limit}:{query}");
    let output = Command::new("yt-dlp")
        .arg("--flat-playlist")
        .arg("--print")
        .arg("%(title)s ||| %(id)s")
        .arg(&search_term)
        .output()
        .await
        .context("failed to run yt-dlp search")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp search failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        if let Some((title, id)) = line.rsplit_once(" ||| ") {
            results.push(SearchResult {
                title: title.trim().to_owned(),
                id: id.trim().to_owned(),
            });
        }
    }

    Ok(results)
}

async fn fetch_video_search_page(query: &str, page: usize, page_size: usize) -> Result<Vec<SearchResult>> {
    let total_limit = page
        .checked_mul(page_size)
        .ok_or_else(|| anyhow!("search pagination overflow"))?;
    let all_results = fetch_video_search_results(query, total_limit).await?;
    let start = (page - 1) * page_size;

    if start >= all_results.len() {
        return Ok(Vec::new());
    }

    Ok(all_results.into_iter().skip(start).take(page_size).collect())
}

async fn select_video_from_paginated_search(query: &str, page_size: usize) -> Result<SearchResult> {
    let mut page = 1;

    loop {
        let results = fetch_video_search_page(query, page, page_size).await?;
        if results.is_empty() {
            if page == 1 {
                bail!("No video results returned by yt-dlp");
            }

            println!("No more results. Returning to previous page.");
            page -= 1;
            continue;
        }

        let start_index = (page - 1) * page_size + 1;
        let end_index = start_index + results.len() - 1;

        println!();
        println!("Video results {start_index}-{end_index}:");
        for (index, result) in results.iter().enumerate() {
            println!("  [{}] {}", index + 1, result.title);
        }

        let prompt = format!("Select [1-{}], n=next, p=prev, q=quit: ", results.len());
        let action = prompt_nonempty(&prompt)?;
        let action_lower = action.to_ascii_lowercase();

        if action_lower == "n" {
            page += 1;
            continue;
        }

        if action_lower == "p" {
            if page > 1 {
                page -= 1;
            } else {
                println!("Already on the first page.");
            }
            continue;
        }

        if action_lower == "q" {
            bail!("Search cancelled by user");
        }

        match action.parse::<usize>() {
            Ok(value) if (1..=results.len()).contains(&value) => {
                return results
                    .get(value - 1)
                    .cloned()
                    .ok_or_else(|| anyhow!("invalid video selection"));
            }
            _ => println!("Enter a valid number, or use n/p/q."),
        }
    }
}

async fn fetch_playlist_search_results(query: &str, limit: usize) -> Result<Vec<PlaylistResult>> {
    let search_term = format!("ytsearch{}:{} playlist", limit * 4, query);
    let output = Command::new("yt-dlp")
        .arg("--flat-playlist")
        .arg("--print")
        .arg("%(title)s ||| %(url)s")
        .arg(&search_term)
        .output()
        .await
        .context("failed to run yt-dlp playlist search")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp playlist search failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        if let Some((title, url)) = line.rsplit_once(" ||| ") {
            let cleaned_url = url.trim();
            if cleaned_url.contains("list=") {
                results.push(PlaylistResult {
                    title: title.trim().to_owned(),
                    url: cleaned_url.to_owned(),
                });
                if results.len() >= limit {
                    break;
                }
            }
        }
    }

    Ok(results)
}

async fn fetch_playlist_videos(playlist_url: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let output = Command::new("yt-dlp")
        .arg("--flat-playlist")
        .arg("--playlist-end")
        .arg(limit.to_string())
        .arg("--print")
        .arg("%(title)s ||| %(id)s")
        .arg(playlist_url)
        .output()
        .await
        .context("failed to fetch playlist videos")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("playlist fetch failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        if let Some((title, id)) = line.rsplit_once(" ||| ") {
            results.push(SearchResult {
                title: title.trim().to_owned(),
                id: id.trim().to_owned(),
            });
            if results.len() >= limit {
                break;
            }
        }
    }

    Ok(results)
}

fn format_filter_for_quality(quality: usize) -> &'static str {
    match quality {
        1 => "best[height<=360][vcodec^=avc1][acodec!=none]/best[height<=360]",
        2 => "best[height<=480][vcodec^=avc1][acodec!=none]/best[height<=480]",
        3 => "best[height<=720][fps<=30][vcodec^=avc1][acodec!=none]/best[height<=720][fps<=30]",
        4 => "best[height<=1080][vcodec^=avc1][acodec!=none]/best[height<=1080]",
        _ => "best[height<=720][fps<=30][vcodec^=avc1][acodec!=none]/best[height<=720][fps<=30]",
    }
}

async fn resolve_stream_url(watch_url: &str, format_filter: &str) -> Result<String> {
    let output = Command::new("yt-dlp")
        .arg("-g")
        .arg("-f")
        .arg(format_filter)
        .arg(watch_url)
        .output()
        .await
        .context("failed to resolve stream URL")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("stream extraction failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stream_url = stdout.lines().next().unwrap_or_default().trim();
    if stream_url.is_empty() {
        bail!("yt-dlp did not return a stream URL");
    }

    Ok(stream_url.to_owned())
}

async fn download_subtitle_file(
    watch_url: &str,
    video_id: &str,
    language: &str,
) -> Result<Option<PathBuf>> {
    let mut base = std::env::temp_dir();
    base.push(format!("custom_yt_sub_{video_id}"));
    let output_template = base.to_string_lossy().to_string();

    let output = Command::new("yt-dlp")
        .arg("--skip-download")
        .arg("--write-sub")
        .arg("--write-auto-sub")
        .arg("--sub-langs")
        .arg(language)
        .arg("--sub-format")
        .arg("vtt")
        .arg("-o")
        .arg(&output_template)
        .arg(watch_url)
        .output()
        .await
        .context("failed to fetch subtitles with yt-dlp")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("Subtitles unavailable: {stderr}");
        return Ok(None);
    }

    let parent = base
        .parent()
        .ok_or_else(|| anyhow!("failed to locate subtitle temp directory"))?;
    let stem = base
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("failed to build subtitle temp filename"))?;

    for entry in fs::read_dir(parent).context("failed to read subtitle temp directory")? {
        let entry = entry.context("failed to inspect subtitle temp file")?;
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if file_name.starts_with(stem) && file_name.ends_with(".vtt") {
            return Ok(Some(path));
        }
    }

    println!("Subtitles were requested, but no .vtt file was produced.");
    Ok(None)
}

async fn launch_mpv(
    stream_url: &str,
    title: &str,
    subtitle_file: Option<&Path>,
) -> Result<()> {
    let window_title = sanitize_window_title(title);

    let mut mpv = Command::new("mpv");
    mpv.arg("--no-config")
        .arg("--force-window=immediate")
        .arg("--title")
        .arg(&window_title)
        .arg("--cache=yes")
        .arg("--demuxer-max-bytes=15MiB") // Capped memory cache (Ideal for 2GB RAM)
        .arg("--vd-lavc-threads=auto")
        .arg("--user-agent=Mozilla/5.0 (X11; Linux x86_64)");

    // Pass subtitle file as a sidecar track for hardware-accelerated overlay
    if let Some(path) = subtitle_file {
        mpv.arg(format!("--sub-file={}", path.to_string_lossy()));
    }

    let mut child = mpv
        .arg(stream_url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to launch mpv")?;

    tokio::select! {
        status = child.wait() => {
            let status = status.context("failed to wait on mpv child process")?;
            if !status.success() {
                bail!("mpv exited with status {status}");
            }
            println!("Playback finished.");
        }
        _ = signal::ctrl_c() => {
            println!("\nReceived Ctrl+C interrupt. Terminating mpv...");
            let _ = child.kill().await;
        }
    }

    Ok(())
}

fn sanitize_window_title(title: &str) -> String {
    let cleaned = title.replace(['\n', '\r'], " ");
    if cleaned.is_empty() {
        String::from("YouTube Playback")
    } else {
        cleaned
    }
}