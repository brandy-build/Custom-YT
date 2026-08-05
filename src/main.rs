use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tokio::signal;

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

/// Where a `PlaybackQueue`'s items came from, and enough state to fetch
/// more of them on demand when the user asks for "next" past the end of
/// what's currently loaded.
#[derive(Debug, Clone)]
enum QueueSource {
    Search {
        query: String,
        page: usize,
        page_size: usize,
    },
    Playlist {
        url: String,
        batch_size: usize,
    },
}

/// A resolved, ordered list of videos plus a cursor into it. This is what
/// makes "next video" possible without re-running the program: instead of
/// picking one video and exiting, we hang on to the full result set (and
/// how to fetch more of it) so the post-playback menu can just move the
/// cursor forward.
struct PlaybackQueue {
    items: Vec<SearchResult>,
    index: usize,
    source: QueueSource,
}

impl PlaybackQueue {
    fn current(&self) -> &SearchResult {
        &self.items[self.index]
    }

    /// Moves to the next item if one is already loaded. If the queue is
    /// exhausted, tries to fetch more (next search page, or next batch of
    /// playlist entries) before giving up. Returns `false` only when there
    /// is genuinely nothing left.
    async fn try_advance(&mut self) -> Result<bool> {
        if self.index + 1 < self.items.len() {
            self.index += 1;
            return Ok(true);
        }

        match &mut self.source {
            QueueSource::Search {
                query,
                page,
                page_size,
            } => {
                let next_page = *page + 1;
                let more = fetch_video_search_page(query, next_page, *page_size).await?;
                if more.is_empty() {
                    return Ok(false);
                }
                self.items.extend(more);
                *page = next_page;
                self.index += 1;
                Ok(true)
            }
            QueueSource::Playlist { url, batch_size } => {
                let start = self.items.len() + 1;
                let end = self.items.len() + *batch_size;
                let more = fetch_playlist_videos(url, start, end).await?;
                if more.is_empty() {
                    return Ok(false);
                }
                self.items.extend(more);
                self.index += 1;
                Ok(true)
            }
        }
    }
}

/// Quality/subtitle choices, kept around so "next video" and "replay" reuse
/// them without re-prompting, until the user explicitly asks to change them.
struct PlaybackSettings {
    quality: usize,
    subtitles: bool,
}

impl PlaybackSettings {
    fn format_filter(&self) -> &'static str {
        format_filter_for_quality(self.quality)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    print_startup_banner();
    ensure_required_binaries()?;

    'session: loop {
        println!("Search mode:");
        println!("  [1] Video search");
        println!("  [2] Playlist search");
        let search_mode = prompt_choice_with_default("Select mode [1-2, default 1]: ", 1, 2, 1)?;

        let query = prompt_nonempty("Search YouTube: ")?;

        let queue_choice = if search_mode == 1 {
            build_search_queue(&query, 10).await?
        } else {
            build_playlist_queue(&query).await?
        };

        let Some(mut queue) = queue_choice else {
            // User backed out of selection (pressed q) - go pick a new search.
            continue 'session;
        };

        let mut settings = prompt_quality_and_subtitles()?;
        let mut loop_video = false;

        'playback: loop {
            println!();
            println!(
                "Now playing: {} {}",
                queue.current().title,
                if loop_video { "[Looping]" } else { "" }
            );

            if let Err(err) = play_current(&queue, &settings, loop_video).await {
                println!("Playback error: {err:#}");
            }

            loop {
                println!();
                println!(
                    "[n] Next video   [r] Replay   [c] Change quality/subtitles   [l] Toggle loop: {}   [s] New search   [q] Quit",
                    if loop_video { "ON" } else { "OFF" }
                );
                let action = prompt_nonempty("Choose an option: ")?.to_ascii_lowercase();

                match action.as_str() {
                    "n" => {
                        if queue.try_advance().await? {
                            continue 'playback;
                        } else {
                            println!("No more videos in this queue.");
                        }
                    }
                    "r" => continue 'playback,
                    "c" => {
                        settings = prompt_quality_and_subtitles()?;
                        continue 'playback;
                    }
                    "l" => {
                        loop_video = !loop_video;
                        continue 'playback;
                    }
                    "s" => continue 'session,
                    "q" => break 'session,
                    _ => println!("Enter n, r, c, l, s, or q."),
                }
            }
        }
    }

    Ok(())
}

/// Runs the search-mode selection flow (with n/p pagination) and returns a
/// ready-to-play queue anchored at the chosen video, or `None` if the user
/// backed out with `q`.
async fn build_search_queue(query: &str, page_size: usize) -> Result<Option<PlaybackQueue>> {
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

        let prompt = format!(
            "Select [1-{}], n=next page, p=prev page, q=quit: ",
            results.len()
        );
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
            return Ok(None);
        }

        match action.parse::<usize>() {
            Ok(value) if (1..=results.len()).contains(&value) => {
                return Ok(Some(PlaybackQueue {
                    items: results,
                    index: value - 1,
                    source: QueueSource::Search {
                        query: query.to_owned(),
                        page,
                        page_size,
                    },
                }));
            }
            _ => println!("Enter a valid number, or use n/p/q."),
        }
    }
}

/// Runs the playlist-mode selection flow and returns a ready-to-play queue
/// anchored at the chosen starting video, or `None` if the user backed out.
async fn build_playlist_queue(query: &str) -> Result<Option<PlaybackQueue>> {
    const PLAYLIST_BATCH_SIZE: usize = 15;

    let playlists = fetch_playlist_search_results(query, 5).await?;
    if playlists.is_empty() {
        println!("No playlist results found. Try a different query or include 'playlist' in search text.");
        return Ok(None);
    }

    println!();
    println!("Playlist results:");
    for (index, playlist) in playlists.iter().enumerate() {
        println!("  [{}] {}", index + 1, playlist.title);
    }

    let playlist_prompt = format!("Select a playlist [1-{}], q=quit: ", playlists.len());
    let playlist_action = prompt_nonempty(&playlist_prompt)?;
    if playlist_action.eq_ignore_ascii_case("q") {
        return Ok(None);
    }
    let playlist_choice: usize = playlist_action
        .parse()
        .ok()
        .filter(|value| (1..=playlists.len()).contains(value))
        .ok_or_else(|| anyhow!("invalid playlist selection"))?;

    let selected_playlist = playlists
        .get(playlist_choice - 1)
        .cloned()
        .ok_or_else(|| anyhow!("invalid playlist selection"))?;

    let playlist_videos =
        fetch_playlist_videos(&selected_playlist.url, 1, PLAYLIST_BATCH_SIZE).await?;
    if playlist_videos.is_empty() {
        println!("Selected playlist has no playable videos.");
        return Ok(None);
    }

    println!();
    println!("Videos in \"{}\":", selected_playlist.title);
    for (index, result) in playlist_videos.iter().enumerate() {
        println!("  [{}] {}", index + 1, result.title);
    }

    let video_choice_prompt = format!("Select a starting video [1-{}]: ", playlist_videos.len());
    let video_choice = prompt_choice(&video_choice_prompt, 1, playlist_videos.len())?;

    Ok(Some(PlaybackQueue {
        items: playlist_videos,
        index: video_choice - 1,
        source: QueueSource::Playlist {
            url: selected_playlist.url,
            batch_size: PLAYLIST_BATCH_SIZE,
        },
    }))
}

fn prompt_quality_and_subtitles() -> Result<PlaybackSettings> {
    println!();
    println!("Quality presets:");
    println!("  [1] 360p  (Ultra Light - Low CPU)");
    println!("  [2] 480p  (Balanced - Fast)");
    println!("  [3] 720p  (HD - High Quality) [Default]");
    println!("  [4] 1080p (Full HD - High Performance)");

    let quality = prompt_choice_with_default("Select a quality [1-4, default 3]: ", 1, 4, 3)?;
    let subtitle_mode = prompt_choice_with_default(
        "Subtitles [1=off, 2=English auto/manual, default 1]: ",
        1,
        2,
        1,
    )?;

    Ok(PlaybackSettings {
        quality,
        subtitles: subtitle_mode == 2,
    })
}

// Gemini: Purpose & Solution - Replaced mpv with ffplay architecture.
// First fetches direct stream URLs via `yt-dlp -g` to decouple media playback from YouTube extraction hooks, eliminating sub-process / Lua script exit errors on Windows.
async fn play_current(
    queue: &PlaybackQueue,
    settings: &PlaybackSettings,
    loop_video: bool,
) -> Result<()> {
    let selected = queue.current();
    let watch_url = format!("https://www.youtube.com/watch?v={}", selected.id);

    let subtitle_file = if settings.subtitles {
        println!("Fetching subtitles...");
        download_subtitle_file(&watch_url, &selected.id, "en").await?
    } else {
        None
    };

    println!("Resolving direct stream URL...");
    let (video_url, audio_url) = resolve_stream_urls(&watch_url, settings.format_filter()).await?;

    println!("Launching playback via ffplay...");
    let playback = launch_ffplay(
        &video_url,
        audio_url.as_deref(),
        &selected.title,
        subtitle_file.as_deref(),
        loop_video,
    )
    .await;

    if let Some(path) = subtitle_file {
        let _ = fs::remove_file(path);
    }

    playback
}

// Gemini: Purpose & Solution - Directly queries `yt-dlp -g` for raw stream links.
// YouTube DASH streams return separate lines for video and audio URLs; this helper splits them so ffplay can load both simultaneously.
async fn resolve_stream_urls(
    watch_url: &str,
    format_filter: &str,
) -> Result<(String, Option<String>)> {
    let output = Command::new("yt-dlp")
        .arg("-g")
        .arg("-f")
        .arg(format_filter)
        .arg(watch_url)
        .output()
        .await
        .context("failed to execute yt-dlp to resolve stream URLs")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp stream extraction failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let urls: Vec<&str> = stdout
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();

    if urls.is_empty() {
        bail!("yt-dlp returned no stream URLs");
    }

    let video_url = urls[0].to_string();
    let audio_url = if urls.len() > 1 {
        Some(urls[1].to_string())
    } else {
        None
    };

    Ok((video_url, audio_url))
}

// Gemini: Purpose & Solution - Spawns `ffplay` with direct stream URLs.
// ffplay is lightweight (~30–50 MB RAM), executes natively on SDL2 without middleman hooks, and auto-exits on stream completion (`-autoexit`).
async fn launch_ffplay(
    video_url: &str,
    audio_url: Option<&str>,
    title: &str,
    subtitle_file: Option<&Path>,
    loop_video: bool,
) -> Result<()> {
    let window_title = sanitize_window_title(title);

    let mut ffplay = Command::new("ffplay");

    // If looping, we don't want autoexit
    if loop_video {
        ffplay.arg("-loop").arg("0");
    } else {
        ffplay.arg("-autoexit");
    }

    ffplay
        .arg("-loglevel")
        .arg("error")
        .arg("-window_title")
        .arg(&window_title)
        .arg("-i")
        .arg(video_url);

    if let Some(audio) = audio_url {
        ffplay.arg("-i").arg(audio);
    }

    // Apply subtitle overlay if subtitles were downloaded
    if let Some(sub_path) = subtitle_file {
        let path_str = sub_path
            .to_string_lossy()
            .replace('\\', "/")
            .replace(':', "\\:");
        ffplay.arg("-vf").arg(format!("subtitles='{}'", path_str));
    }

    let mut child = ffplay
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to launch ffplay")?;

    tokio::select! {
        status = child.wait() => {
            let status = status.context("failed to wait on ffplay child process")?;
            if !status.success() {
                bail!("ffplay exited with status {status}");
            }
            println!("Playback finished.");
        }
        _ = signal::ctrl_c() => {
            println!("\nReceived Ctrl+C interrupt. Terminating ffplay...");
            let _ = child.kill().await;
        }
    }

    Ok(())
}

/// Checks that `yt-dlp` and `ffplay` are runnable, and prints an install hint
/// tailored to the current OS if either is missing.
fn ensure_required_binaries() -> Result<()> {
    let required = ["yt-dlp", "ffplay"];
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|binary| !binary_is_runnable(binary))
        .collect();

    if missing.is_empty() {
        return Ok(());
    }

    println!(
        "Missing or unreachable required binaries: {}",
        missing.join(", ")
    );
    println!();
    println!("{}", install_hint_for_current_os());
    println!();
    println!(
        "If you just installed these, close and reopen your terminal (and IDE, if applicable) \
         so it picks up the updated PATH, then try again."
    );

    bail!("required system binaries are missing or unreachable");
}

/// Returns an OS-appropriate install command block for missing dependencies.
fn install_hint_for_current_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "Install them on Windows with one of:\n\n  \
         winget install yt-dlp.yt-dlp\n  \
         winget install Gyan.FFmpeg\n\n\
         Then confirm they're resolvable with:\n\n  \
         where.exe ffplay\n  where.exe yt-dlp"
    } else if cfg!(target_os = "macos") {
        "Install them on macOS with:\n\n  \
         brew install ffmpeg yt-dlp\n\n\
         Then confirm they're resolvable with:\n\n  \
         which ffplay\n  which yt-dlp"
    } else {
        "Install them on Debian/antiX/Ubuntu with:\n\n  \
         sudo apt update && sudo apt install -y ffmpeg yt-dlp\n\n\
         Then confirm they're resolvable with:\n\n  \
         which ffplay\n  which yt-dlp"
    }
}

/// Tries to invoke `<name> -version` to verify presence on system PATH.
fn binary_is_runnable(name: &str) -> bool {
    use std::sync::mpsc;
    use std::time::Duration;

    let (tx, rx) = mpsc::channel();
    let name_owned = name.to_owned();

    std::thread::spawn(move || {
        let result = std::process::Command::new(&name_owned)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok();
        let _ = tx.send(result);
    });

    rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false)
}

fn print_startup_banner() {
    let reset = "\x1b[0m";
    let cyan = "\x1b[36m";
    let white_bold = "\x1b[1;37m";
    let dim = "\x1b[2;37m";

    let banner = include_str!("../ascii-art.txt");
    let lines: Vec<&str> = banner.lines().collect();

    // Simple animation: move left-to-right
    for offset in 0..10 {
        print!("\x1b[2J\x1b[H"); // Clear screen and home cursor
        for line in &lines {
            println!("{}{:>width$}{}", cyan, "", line, width = offset);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    println!();
    println!(
        "{}CuttleFish{} - Minimalist YouTube Terminal Player",
        white_bold, reset
    );
    println!();
    println!("{}Command Manual{}", white_bold, reset);
    println!("  {}1.{} Enter a search query.", dim, reset);
    println!("  {}2.{} Pick video mode or playlist mode.", dim, reset);
    println!(
        "  {}3.{} In video mode, use n/p paging and pick from visible 10 results.",
        dim, reset
    );
    println!(
        "  {}4.{} Choose a quality preset, or press Enter for 720p.",
        dim, reset
    );
    println!("  {}5.{} Optionally enable subtitles.", dim, reset);
    println!(
        "  {}6.{} ffplay streams the direct media URLs seamlessly.",
        dim, reset
    );
    println!(
        "  {}7.{} After playback: n = next video, r = replay, c = change quality, l = toggle loop, s = new search, q = quit.",
        dim, reset
    );
    println!();
    println!("{}Architecture{}", white_bold, reset);
    println!("  {}input -> yt-dlp search -> result selection -> direct URL resolution -> ffplay -> next/replay/search{}", dim, reset);
    println!("{}Design{}", white_bold, reset);
    println!(
        "  {}terminal-first, ultra-low-RAM (~40MB), bulletproof process execution{}",
        dim, reset
    );
    println!();
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
            _ => {
                println!("Enter a number between {min} and {max}, or press Enter for the default.")
            }
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

async fn fetch_video_search_page(
    query: &str,
    page: usize,
    page_size: usize,
) -> Result<Vec<SearchResult>> {
    let total_limit = page
        .checked_mul(page_size)
        .ok_or_else(|| anyhow!("search pagination overflow"))?;
    let all_results = fetch_video_search_results(query, total_limit).await?;
    let start = (page - 1) * page_size;

    if start >= all_results.len() {
        return Ok(Vec::new());
    }

    Ok(all_results
        .into_iter()
        .skip(start)
        .take(page_size)
        .collect())
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

/// Fetches a 1-indexed, inclusive `[start, end]` range of a playlist's videos.
async fn fetch_playlist_videos(
    playlist_url: &str,
    start: usize,
    end: usize,
) -> Result<Vec<SearchResult>> {
    let output = Command::new("yt-dlp")
        .arg("--flat-playlist")
        .arg("--playlist-start")
        .arg(start.to_string())
        .arg("--playlist-end")
        .arg(end.to_string())
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
        }
    }

    Ok(results)
}

fn format_filter_for_quality(quality: usize) -> &'static str {
    match quality {
        1 => "best[height<=360][vcodec^=avc1][acodec!=none]/best[height<=360]",
        2 => "best[height<=480][vcodec^=avc1][acodec!=none]/best[height<=480]",
        3 => "best[height<=720][fps<=30][vcodec^=avc1][acodec!=none]/best[height<=720][fps<=30]",
        4 => "bestvideo[height<=1080]+bestaudio/best[height<=1080]",
        _ => "best[height<=720][fps<=30][vcodec^=avc1][acodec!=none]/best[height<=720][fps<=30]",
    }
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

fn sanitize_window_title(title: &str) -> String {
    let cleaned = title.replace(['\n', '\r'], " ");
    if cleaned.is_empty() {
        String::from("YouTube Playback")
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_window_title() {
        assert_eq!(sanitize_window_title(""), "YouTube Playback");
        assert_eq!(sanitize_window_title("Hello\nWorld"), "Hello World");
        assert_eq!(sanitize_window_title("Hello\rWorld"), "Hello World");
        assert_eq!(sanitize_window_title("Hello\r\nWorld"), "Hello  World");
        assert_eq!(sanitize_window_title("Regular Title"), "Regular Title");
    }

    #[test]
    fn test_format_filter_for_quality() {
        assert_eq!(
            format_filter_for_quality(1),
            "best[height<=360][vcodec^=avc1][acodec!=none]/best[height<=360]"
        );
        assert_eq!(
            format_filter_for_quality(2),
            "best[height<=480][vcodec^=avc1][acodec!=none]/best[height<=480]"
        );
        assert_eq!(
            format_filter_for_quality(3),
            "best[height<=720][fps<=30][vcodec^=avc1][acodec!=none]/best[height<=720][fps<=30]"
        );
        assert_eq!(
            format_filter_for_quality(4),
            "bestvideo[height<=1080]+bestaudio/best[height<=1080]"
        );
        assert_eq!(
            format_filter_for_quality(5), // fallback
            "best[height<=720][fps<=30][vcodec^=avc1][acodec!=none]/best[height<=720][fps<=30]"
        );
    }

    #[test]
    fn test_playback_settings_format_filter() {
        let settings = PlaybackSettings {
            quality: 2,
            subtitles: false,
        };
        assert_eq!(settings.format_filter(), format_filter_for_quality(2));
    }

    #[test]
    fn test_playback_queue_current() {
        let items = vec![
            SearchResult {
                title: "Video 1".to_string(),
                id: "id1".to_string(),
            },
            SearchResult {
                title: "Video 2".to_string(),
                id: "id2".to_string(),
            },
        ];
        let queue = PlaybackQueue {
            items,
            index: 0,
            source: QueueSource::Search {
                query: "test".to_string(),
                page: 1,
                page_size: 10,
            },
        };
        assert_eq!(queue.current().title, "Video 1");
        assert_eq!(queue.current().id, "id1");
    }

    #[tokio::test]
    async fn test_playback_queue_try_advance_in_memory() {
        let items = vec![
            SearchResult {
                title: "Video 1".to_string(),
                id: "id1".to_string(),
            },
            SearchResult {
                title: "Video 2".to_string(),
                id: "id2".to_string(),
            },
        ];
        let mut queue = PlaybackQueue {
            items,
            index: 0,
            source: QueueSource::Search {
                query: "test".to_string(),
                page: 1,
                page_size: 10,
            },
        };

        // Advance to index 1 (should not hit the network since index + 1 < items.len())
        let advanced = queue.try_advance().await.unwrap();
        assert!(advanced);
        assert_eq!(queue.index, 1);
        assert_eq!(queue.current().title, "Video 2");
    }

    #[test]
    fn test_binary_is_runnable_for_nonexistent() {
        assert!(!binary_is_runnable("non_existent_binary_name_xyz"));
    }

    #[test]
    fn test_install_hint_for_current_os() {
        let hint = install_hint_for_current_os();
        assert!(!hint.is_empty());
        if cfg!(target_os = "windows") {
            assert!(hint.contains("winget install"));
        } else if cfg!(target_os = "macos") {
            assert!(hint.contains("brew install"));
        } else {
            assert!(hint.contains("apt install"));
        }
    }
}
