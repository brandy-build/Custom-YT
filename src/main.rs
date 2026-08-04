use anyhow::{anyhow, bail, Context, Result};
use std::io::{self, Write};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Clone)]
struct SearchResult {
    title: String,
    id: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    print_startup_banner();
    ensure_required_binaries()?;

    let query = prompt_nonempty("Search YouTube: ")?;
    let results = fetch_search_results(&query).await?;

    if results.is_empty() {
        bail!("No results returned by yt-dlp");
    }

    println!();
    println!("Top results:");
    for (index, result) in results.iter().enumerate() {
        println!("  [{}] {}", index + 1, result.title);
    }

    let selection = prompt_choice("Select a video [1-5]: ", 1, results.len())?;
    let selected = results
        .get(selection - 1)
        .cloned()
        .ok_or_else(|| anyhow!("invalid video selection"))?;

    println!();
    println!("Quality presets:");
    println!("  [1] 360p  (Ultra Light - Low CPU)");
    println!("  [2] 480p  (Balanced - Fast)");
    println!("  [3] 720p  (HD - High Quality) [Default]");
    println!("  [4] 1080p (Full HD - High Performance)");

    let quality = prompt_choice_with_default("Select a quality [1-4, default 3]: ", 1, 4, 3)?;
    let format_filter = format_filter_for_quality(quality);

    println!();
    println!("Resolving stream URL...");
    let watch_url = format!("https://www.youtube.com/watch?v={}", selected.id);
    let stream_url = resolve_stream_url(&watch_url, &format_filter).await?;

    println!("Launching playback...");
    launch_ffplay(&stream_url, &selected.title).await?;

    Ok(())
}

fn ensure_required_binaries() -> Result<()> {
    let required = ["yt-dlp", "ffplay"];
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
        "Install them on Debian/antiX/Ubuntu with:\n\n  sudo apt update && sudo apt install --no-install-recommends -y ffmpeg yt-dlp build-essential"
    );
    bail!("required system binaries are missing");
}

fn print_startup_banner() {
    let banner = include_str!("../ascii-art.txt").trim_matches('\n');
    println!("{banner}");

    println!("Command Manual");
    println!("  1. Enter a search query.");
    println!("  2. Pick one of the top 5 results.");
    println!("  3. Choose a quality preset, or press Enter for 720p.");
    println!("  4. Wait while ffplay plays the selected video.");
    println!();
    println!("Architecture");
    println!("  input -> yt-dlp search -> result selection -> stream extraction -> ffplay");
    println!("Design");
    println!("  terminal-first, low-RAM, AVC-preferred, and minimal-dependency by design");
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

async fn fetch_search_results(query: &str) -> Result<Vec<SearchResult>> {
    let search_term = format!("ytsearch5:{query}");
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

fn format_filter_for_quality(quality: usize) -> &'static str {
    match quality {
        1 => "best[height<=360][vcodec^=avc1]/bestvideo[height<=360][vcodec^=avc1]+bestaudio/best[height<=360]",
        2 => "best[height<=480][vcodec^=avc1]/bestvideo[height<=480][vcodec^=avc1]+bestaudio/best[height<=480]",
        3 => "best[height<=720][vcodec^=avc1]/bestvideo[height<=720][vcodec^=avc1]+bestaudio/best[height<=720]",
        4 => "best[height<=1080][vcodec^=avc1]/bestvideo[height<=1080][vcodec^=avc1]+bestaudio/best[height<=1080]",
        _ => "best[height<=720][vcodec^=avc1]/bestvideo[height<=720][vcodec^=avc1]+bestaudio/best[height<=720]",
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

async fn launch_ffplay(stream_url: &str, title: &str) -> Result<()> {
    let window_title = sanitize_window_title(title);

    let status = Command::new("ffplay")
        .arg("-user_agent")
        .arg("Mozilla/5.0 (X11; Linux x86_64)")
        .arg("-infbuf")
        .arg("-autoexit")
        .arg("-framedrop")
        .arg("-fast")
        .arg("-probesize")
        .arg("5000000")
        .arg("-analyzeduration")
        .arg("2000000")
        .arg("-threads")
        .arg("auto")
        .arg("-window_title")
        .arg(&window_title)
        .arg("-i")
        .arg(stream_url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("failed to launch ffplay")?;

    if !status.success() {
        bail!("ffplay exited with status {status}");
    }

    println!("Playback finished.");
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
