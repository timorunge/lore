use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use regex::Regex;
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::ingest::types::{FailedDoc, LoaderResult};
use crate::types::{DocKind, DocMeta, SourceId, SourceType};
use crate::util::progress::ProgressHandle;

/// Minimum accumulated characters before inserting a paragraph break when
/// joining transcript segments into a single string for the chunker.
const TRANSCRIPT_PARAGRAPH_BREAK_CHARS: usize = 500;

/// Classified YouTube URL target.
pub(crate) enum YoutubeTarget {
    Video(String),
    Playlist,
    Channel,
}

/// Metadata extracted from yt-dlp video info JSON.
struct VideoMeta {
    keywords: Vec<String>,
    title: Option<String>,
    author: Option<String>,
    description: Option<String>,
    lang: Option<String>,
}

/// Run a yt-dlp command with a timeout, capturing stdout and returning it trimmed.
async fn run_ytdlp(args: &[&str], cwd: &Path, timeout_secs: u64) -> Result<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::process::Command::new("yt-dlp")
            .args(args)
            .current_dir(cwd)
            .output(),
    )
    .await
    .context("yt-dlp operation timed out")?
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "yt-dlp is not installed; youtube sources require yt-dlp on PATH \
                 (https://github.com/yt-dlp/yt-dlp)"
            )
        } else {
            anyhow::Error::from(e).context("failed to run yt-dlp")
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("yt-dlp {:?} failed: {}", args, stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout).context("yt-dlp output is not valid UTF-8")?;
    Ok(stdout)
}

/// Fetch video metadata as JSON via `yt-dlp --dump-json`.
async fn ytdlp_dump_json(url: &str, timeout_secs: u64) -> Result<Value> {
    let tmp = tempfile::tempdir().context("failed to create temp dir")?;
    let stdout = run_ytdlp(
        &["--dump-json", "--no-playlist", url],
        tmp.path(),
        timeout_secs,
    )
    .await?;
    serde_json::from_str(stdout.trim()).context("failed to parse yt-dlp JSON output")
}

/// List video IDs from a playlist or channel via `yt-dlp --flat-playlist`.
/// Returns one video ID per entry, capped at `max`.
pub(crate) async fn ytdlp_flat_playlist(
    url: &str,
    max: usize,
    timeout_secs: u64,
) -> Result<Vec<String>> {
    let tmp = tempfile::tempdir().context("failed to create temp dir")?;
    let max_str = max.to_string();
    let stdout = run_ytdlp(
        &[
            "--flat-playlist",
            "--dump-json",
            "--playlist-end",
            &max_str,
            url,
        ],
        tmp.path(),
        timeout_secs,
    )
    .await?;

    let mut ids = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(obj) = serde_json::from_str::<Value>(line)
            && let Some(id) = obj["id"].as_str().filter(|s| !s.is_empty())
        {
            ids.push(id.to_owned());
        }
    }
    ids.truncate(max);
    Ok(ids)
}

/// Download subtitles for a single video into `tmp_dir`.
/// Returns the path to the `.srv3` file, or `None` if no subtitles were written.
async fn ytdlp_write_sub(
    url: &str,
    lang: &str,
    tmp_dir: &Path,
    timeout_secs: u64,
) -> Result<Option<PathBuf>> {
    // yt-dlp writes VIDEO_ID.LANG.srv3 into cwd
    let result = run_ytdlp(
        &[
            "--write-sub",
            "--write-auto-sub",
            "--sub-lang",
            lang,
            "--sub-format",
            "srv3",
            "--skip-download",
            "--no-playlist",
            "-o",
            "%(id)s",
            url,
        ],
        tmp_dir,
        timeout_secs,
    )
    .await;

    // yt-dlp exits non-zero when unavailable -- handled upstream.
    // If the command succeeded but no subtitle file was written, that's fine.
    if let Err(e) = &result {
        let msg = format!("{e}");
        if msg.contains("no subtitles") || msg.contains("There are no subtitles") {
            return Ok(None);
        }
        result?;
    }

    for entry in std::fs::read_dir(tmp_dir).context("failed to read subtitle temp dir")? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("srv3") {
            return Ok(Some(path));
        }
    }

    Ok(None)
}

/// Extract video metadata from yt-dlp `--dump-json` output.
fn extract_video_meta(info: &Value) -> VideoMeta {
    let title = info["title"].as_str().map(str::to_owned);
    let author = info["uploader"].as_str().map(str::to_owned);
    let description = info["description"].as_str().map(str::to_owned);
    let keywords = info["tags"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    let lang = first_subtitle_lang(&info["subtitles"])
        .or_else(|| first_subtitle_lang(&info["automatic_captions"]));

    VideoMeta {
        keywords,
        title,
        author,
        description,
        lang,
    }
}

/// Get the first language code from a subtitles/automatic_captions object.
fn first_subtitle_lang(obj: &Value) -> Option<String> {
    obj.as_object()
        .and_then(|m| m.keys().next().map(std::borrow::ToOwned::to_owned))
}

/// Select the best subtitle language from yt-dlp info JSON.
///
/// Priority: manual exact > manual prefix > ASR exact > ASR prefix > any manual > any ASR.
/// Returns `(language_code, is_asr)`.
fn select_subtitle_lang(info: &Value, preferred_lang: &str) -> Option<(String, bool)> {
    // Use references to the JSON maps directly to avoid allocating intermediate
    // Vec<String> -- we only need to iterate and clone at most one key.
    let manual = info["subtitles"].as_object();
    let auto = info["automatic_captions"].as_object();

    if manual.is_none_or(serde_json::Map::is_empty) && auto.is_none_or(serde_json::Map::is_empty) {
        return None;
    }

    let pref = preferred_lang.to_lowercase();

    // Strip "-orig" suffix for matching (yt-dlp uses e.g. "en-orig" for original auto captions)
    let normalize = |k: &str| k.to_lowercase().replace("-orig", "");

    if let Some(k) = manual.and_then(|m| m.keys().find(|k| normalize(k) == pref)) {
        return Some((k.clone(), false));
    }
    if let Some(k) = manual.and_then(|m| m.keys().find(|k| normalize(k).starts_with(&pref))) {
        return Some((k.clone(), false));
    }
    if let Some(k) = auto.and_then(|m| m.keys().find(|k| normalize(k) == pref)) {
        return Some((k.clone(), true));
    }
    if let Some(k) = auto.and_then(|m| m.keys().find(|k| normalize(k).starts_with(&pref))) {
        return Some((k.clone(), true));
    }
    if let Some(k) = manual.and_then(|m| m.keys().next()) {
        return Some((k.clone(), false));
    }
    auto.and_then(|m| m.keys().next())
        .map(|k| (k.clone(), true))
}

/// Classify a YouTube URL into video, playlist, or channel.
pub(crate) fn parse_youtube_url(url: &str) -> Result<YoutubeTarget> {
    // youtu.be/VIDEO_ID
    if let Some(rest) = url
        .strip_prefix("https://youtu.be/")
        .or_else(|| url.strip_prefix("http://youtu.be/"))
    {
        let id = rest.split(['?', '&', '/']).next().unwrap_or(rest);
        anyhow::ensure!(!id.is_empty(), "empty video ID in youtu.be URL");
        return Ok(YoutubeTarget::Video(id.to_owned()));
    }

    let parsed = url::Url::parse(url).context("invalid YouTube URL")?;
    let host = parsed.host_str().unwrap_or("");
    anyhow::ensure!(
        host == "www.youtube.com"
            || host == "youtube.com"
            || host == "m.youtube.com"
            || host == "youtu.be",
        "not a YouTube URL: {url}"
    );

    let path = parsed.path();

    // /watch?v=VIDEO_ID
    if path == "/watch"
        && let Some(v) = parsed
            .query_pairs()
            .find(|(k, _)| k == "v")
            .map(|(_, v)| v.into_owned())
    {
        anyhow::ensure!(!v.is_empty(), "empty video ID in watch URL");
        return Ok(YoutubeTarget::Video(v));
    }

    // /playlist?list=PLAYLIST_ID
    if path == "/playlist"
        && let Some(list) = parsed
            .query_pairs()
            .find(|(k, _)| k == "list")
            .map(|(_, v)| v.into_owned())
    {
        anyhow::ensure!(!list.is_empty(), "empty playlist ID");
        return Ok(YoutubeTarget::Playlist);
    }

    // /shorts/VIDEO_ID
    if let Some(rest) = path.strip_prefix("/shorts/") {
        let id = rest.split('/').next().unwrap_or(rest);
        anyhow::ensure!(!id.is_empty(), "empty video ID in shorts URL");
        return Ok(YoutubeTarget::Video(id.to_owned()));
    }

    // /@handle or /channel/UCID or /c/name
    if path.starts_with("/@") {
        return Ok(YoutubeTarget::Channel);
    }
    if let Some(rest) = path.strip_prefix("/channel/") {
        let channel_id = rest.split('/').next().unwrap_or(rest);
        anyhow::ensure!(!channel_id.is_empty(), "empty channel ID");
        return Ok(YoutubeTarget::Channel);
    }
    if let Some(rest) = path.strip_prefix("/c/") {
        let name = rest.split('/').next().unwrap_or(rest);
        anyhow::ensure!(!name.is_empty(), "empty channel name");
        return Ok(YoutubeTarget::Channel);
    }

    // Fallback: if there's a `v` query param, treat as video
    if let Some(v) = parsed
        .query_pairs()
        .find(|(k, _)| k == "v")
        .map(|(_, v)| v.into_owned())
        && !v.is_empty()
    {
        return Ok(YoutubeTarget::Video(v));
    }

    anyhow::bail!("could not determine YouTube video/playlist/channel from URL: {url}")
}

/// Parse YouTube subtitle XML into plain text.
///
/// Supports two XML formats:
/// - **timedtext**: `<text start="..." dur="...">content</text>`
/// - **srv3** (yt-dlp default): `<p t="..." d="..."><s>word</s> ...</p>`
///
/// Both use the same outer structure -- we collect text from `<text>` and `<p>`
/// elements (including nested `<s>` spans). HTML tags are stripped, entities
/// decoded, and segments joined with paragraph breaks every ~`TRANSCRIPT_PARAGRAPH_BREAK_CHARS` chars.
fn parse_transcript_xml(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    let mut segments: Vec<String> = Vec::new();
    let mut depth: u32 = 0; // nesting depth inside a caption element
    let mut current = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let tag = name.as_ref();
                if tag == b"text" || (tag == b"p" && depth == 0) {
                    depth = 1;
                    current.clear();
                } else if depth > 0 {
                    depth += 1;
                }
            }
            Ok(Event::End(e)) if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let name = e.name();
                    let tag = name.as_ref();
                    if tag == b"text" || tag == b"p" {
                        let cleaned = strip_html_tags(&current);
                        let cleaned = cleaned.trim();
                        if !cleaned.is_empty() {
                            segments.push(cleaned.to_owned());
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let name = e.name();
                let tag = name.as_ref();
                if (tag == b"text" || tag == b"p") && depth == 0 {
                    // Self-closing empty caption -- skip
                }
            }
            Ok(Event::Text(e)) if depth > 0 => {
                if let Ok(text) = e.decode() {
                    current.push_str(&text);
                }
            }
            Ok(Event::GeneralRef(e)) if depth > 0 => {
                if let Ok(Some(ch)) = e.resolve_char_ref() {
                    current.push(ch);
                } else if let Ok(name) = e.decode() {
                    match name.as_ref() {
                        "amp" => current.push('&'),
                        "lt" => current.push('<'),
                        "gt" => current.push('>'),
                        "apos" => current.push('\''),
                        "quot" => current.push('"'),
                        _ => {}
                    }
                }
            }
            Ok(Event::CData(e)) if depth > 0 => {
                current.push_str(&String::from_utf8_lossy(e.as_ref()));
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    // Join segments into paragraphs (~TRANSCRIPT_PARAGRAPH_BREAK_CHARS per paragraph for chunker).
    let mut result = String::new();
    let mut line_len = 0;
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            if line_len > TRANSCRIPT_PARAGRAPH_BREAK_CHARS {
                result.push_str("\n\n");
                line_len = 0;
            } else {
                result.push(' ');
                line_len += 1;
            }
        }
        result.push_str(seg);
        line_len += seg.len();
    }

    result
}

/// Strip HTML tags from a string (e.g. `<b>bold</b>` -> `bold`).
///
/// Edge case: a literal `<` character that is not part of a tag (e.g. in
/// math expressions like `a < b`) will be incorrectly treated as the start
/// of a tag and suppress the following text until the next `>`. This is
/// acceptable for subtitle content, where bare `<` is rare.
fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Main entry point: fetch YouTube transcripts for a configured source.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn fetch_youtube(
    url: &str,
    lang: &str,
    topic: Option<&str>,
    include_re: Option<&Regex>,
    max_videos: usize,
    existing_docs: &HashMap<SourceId, DocMeta>,
    force: bool,
    concurrency: usize,
    timeout_secs: u64,
    progress: &ProgressHandle,
) -> Result<(Vec<LoaderResult>, Vec<FailedDoc>)> {
    let target = parse_youtube_url(url)?;
    progress.inc_length(1);
    let video_ids = discover_video_ids(url, &target, max_videos, timeout_secs).await?;

    info!(
        url,
        videos = video_ids.len(),
        "discovered YouTube video IDs"
    );

    progress.inc_length(video_ids.len() as u64);
    progress.inc(1);

    let progress_clone = progress.clone();
    let failures: Mutex<Vec<FailedDoc>> = Mutex::new(Vec::new());
    let results: Vec<LoaderResult> = stream::iter(video_ids)
        .map(|video_id| async move {
            let source = format!("https://www.youtube.com/watch?v={video_id}");
            let result = load_single_video(
                &video_id,
                lang,
                topic,
                include_re,
                existing_docs,
                force,
                timeout_secs,
            )
            .await;
            (source, result)
        })
        .buffer_unordered(concurrency)
        .filter_map(|(source, r)| {
            let pb = progress_clone.clone();
            let failures = &failures;
            async move {
                pb.inc(1);
                match r {
                    Ok(Some(doc)) => Some(doc),
                    Ok(None) => None,
                    Err(e) => {
                        failures
                            .lock()
                            .expect("not poisoned")
                            .push(FailedDoc::new(source, format!("{e:#}")));
                        warn!("failed to load YouTube video: {e:#}");
                        None
                    }
                }
            }
        })
        .collect()
        .await;

    let failures = failures.into_inner().expect("not poisoned");
    info!(
        url,
        documents = results.len(),
        failed = failures.len(),
        "loaded YouTube transcripts"
    );
    Ok((results, failures))
}

/// Resolve a YouTube target to a list of video IDs.
async fn discover_video_ids(
    url: &str,
    target: &YoutubeTarget,
    max: usize,
    timeout_secs: u64,
) -> Result<Vec<String>> {
    match target {
        YoutubeTarget::Video(id) => Ok(vec![id.clone()]),
        YoutubeTarget::Playlist | YoutubeTarget::Channel => {
            ytdlp_flat_playlist(url, max, timeout_secs).await
        }
    }
}

/// Fetch metadata, download subtitles, parse the transcript, and return a `LoaderResult` for one video.
async fn load_single_video(
    video_id: &str,
    lang: &str,
    topic: Option<&str>,
    include_re: Option<&Regex>,
    existing_docs: &HashMap<SourceId, DocMeta>,
    force: bool,
    timeout_secs: u64,
) -> Result<Option<LoaderResult>> {
    let source = format!("https://www.youtube.com/watch?v={video_id}");
    let source_id = crate::types::source_id(&source);

    if !force && existing_docs.contains_key(&source_id) {
        debug!(video_id, "skipping already-indexed video");
        return Ok(Some(LoaderResult {
            source_id,
            source,
            origin: SourceType::Youtube,
            kind: DocKind::default(),
            content: String::new(),
            unchanged: true,
            format: None,
            topic: topic.map(str::to_owned),
            title: None,
            author: None,
            lang: None,
            created_at: None,
            tags: None,
            mtime_ns: None,
            size_bytes: None,
            etag: None,
            last_modified: None,
            content_hash_override: None,
        }));
    }

    let info = ytdlp_dump_json(&source, timeout_secs).await?;

    let meta = extract_video_meta(&info);

    // `include_re` is a post-fetch filter applied after yt-dlp has already
    // returned the video metadata. It is NOT a server-side filter; all videos
    // in the playlist are fetched first, then titles are matched locally.
    if let Some(re) = include_re {
        let title = meta.title.as_deref().unwrap_or("");
        if !re.is_match(title) {
            debug!(video_id, title, "filtered out by include regex");
            return Ok(None);
        }
    }

    let Some((sub_lang, is_asr)) = select_subtitle_lang(&info, lang) else {
        warn!(video_id, "no subtitles available");
        return Ok(None);
    };

    debug!(
        video_id,
        lang = sub_lang.as_str(),
        asr = is_asr,
        "selected subtitle language"
    );

    let tmp_dir = tempfile::tempdir().context("failed to create temp dir for subtitles")?;
    let Some(srv3_path) = ytdlp_write_sub(&source, &sub_lang, tmp_dir.path(), timeout_secs).await?
    else {
        warn!(
            video_id,
            lang = sub_lang.as_str(),
            "no subtitle file written"
        );
        return Ok(None);
    };

    let xml = std::fs::read_to_string(&srv3_path).context("failed to read subtitle file")?;
    let transcript = parse_transcript_xml(&xml);
    if transcript.is_empty() {
        warn!(video_id, "transcript is empty");
        return Ok(None);
    }

    let mut header = String::new();
    if let Some(title) = &meta.title {
        header.push_str("# ");
        header.push_str(title);
        header.push_str("\n\n");
    }
    if let Some(author) = &meta.author {
        header.push_str("Author: ");
        header.push_str(author);
        header.push('\n');
    }
    if !meta.keywords.is_empty() {
        header.push_str("Tags: ");
        header.push_str(&meta.keywords.join(", "));
        header.push('\n');
    }
    if let Some(desc) = &meta.description {
        let short = if desc.len() > 500 {
            let mut end = 500;
            while end > 0 && !desc.is_char_boundary(end) {
                end -= 1;
            }
            &desc[..end]
        } else {
            desc.as_str()
        };
        header.push('\n');
        header.push_str(short);
        header.push('\n');
    }
    if !header.is_empty() {
        header.push_str("\n---\n\n");
    }

    let content = format!("{header}{transcript}");

    let tags = if meta.keywords.is_empty() {
        None
    } else {
        Some(meta.keywords.join(", "))
    };

    Ok(Some(LoaderResult {
        source_id,
        source,
        origin: SourceType::Youtube,
        kind: DocKind::Document,
        content,
        unchanged: false,
        format: Some("transcript".to_owned()),
        topic: topic.map(str::to_owned),
        title: meta.title,
        author: meta.author,
        lang: meta.lang.or(Some(sub_lang)),
        created_at: None,
        tags,
        mtime_ns: None,
        size_bytes: None,
        etag: None,
        last_modified: None,
        content_hash_override: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_youtube_url_cases() {
        match parse_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap() {
            YoutubeTarget::Video(id) => assert_eq!(id, "dQw4w9WgXcQ"),
            _ => panic!("expected Video"),
        }

        match parse_youtube_url("https://youtu.be/dQw4w9WgXcQ").unwrap() {
            YoutubeTarget::Video(id) => assert_eq!(id, "dQw4w9WgXcQ"),
            _ => panic!("expected Video"),
        }

        assert!(matches!(
            parse_youtube_url(
                "https://www.youtube.com/playlist?list=PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf"
            )
            .unwrap(),
            YoutubeTarget::Playlist
        ));

        assert!(matches!(
            parse_youtube_url("https://www.youtube.com/@fireship").unwrap(),
            YoutubeTarget::Channel
        ));

        assert!(matches!(
            parse_youtube_url("https://www.youtube.com/channel/UCsBjURrPoezykLs9EqgamOA").unwrap(),
            YoutubeTarget::Channel
        ));

        match parse_youtube_url("https://www.youtube.com/shorts/dQw4w9WgXcQ").unwrap() {
            YoutubeTarget::Video(id) => assert_eq!(id, "dQw4w9WgXcQ"),
            _ => panic!("expected Video"),
        }

        assert!(parse_youtube_url("https://example.com/watch?v=abc").is_err());
    }

    #[test]
    fn parse_transcript_xml_cases() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<transcript>
  <text start="0.0" dur="2.5">Hello world</text>
  <text start="2.5" dur="3.0">This is a &amp; test</text>
  <text start="5.5" dur="2.0"><b>Bold</b> text</text>
</transcript>"#;

        let result = parse_transcript_xml(xml);
        assert!(result.contains("Hello world"));
        assert!(result.contains("This is a & test"));
        assert!(result.contains("Bold text"));
        assert!(!result.contains("<b>"));

        let xml = r#"<?xml version="1.0" encoding="utf-8" ?><timedtext format="3">
<body>
<p t="11840" d="7199" w="1"><s ac="0">So,</s><s t="320" ac="0"> microservices</s><s t="1040" ac="0"> have</s></p>
<p t="13759" d="7121" w="1"><s ac="0">a</s><s t="241" ac="0"> lot</s><s t="960" ac="0"> over</s><s t="1441" ac="0"> the</s></p>
<p t="23279" d="3201" w="1"><s ac="0">it&#39;s</s><s t="561" ac="0"> something</s></p>
</body></timedtext>"#;

        let result = parse_transcript_xml(xml);
        assert!(result.contains("So, microservices have"));
        assert!(result.contains("a lot over the"));
        assert!(result.contains("it's something"));
    }

    #[test]
    fn strip_html_tags_works() {
        assert_eq!(strip_html_tags("<b>bold</b>"), "bold");
        assert_eq!(strip_html_tags("<i>a</i> <b>b</b>"), "a b");
        assert_eq!(strip_html_tags("no tags here"), "no tags here");
        assert_eq!(strip_html_tags(""), "");
        assert_eq!(strip_html_tags("<div class=\"x\">inner</div>"), "inner",);
        assert_eq!(strip_html_tags("<br/>text"), "text");
        assert_eq!(strip_html_tags("<p><b>nested</b> text</p>"), "nested text",);
    }

    #[test]
    fn extract_video_meta_cases() {
        let info = serde_json::json!({
            "title": "My Video",
            "uploader": "Some Channel",
            "description": "A great video about things",
            "tags": ["rust", "programming"],
            "subtitles": {"en": []},
            "automatic_captions": {"en": [], "fr": []},
        });

        let meta = extract_video_meta(&info);
        assert_eq!(meta.title.as_deref(), Some("My Video"));
        assert_eq!(meta.author.as_deref(), Some("Some Channel"));
        assert_eq!(
            meta.description.as_deref(),
            Some("A great video about things")
        );
        assert_eq!(meta.keywords, vec!["rust", "programming"]);
        assert_eq!(meta.lang.as_deref(), Some("en"));

        let info = serde_json::json!({
            "title": "Test",
            "automatic_captions": {"de": []},
        });

        let meta = extract_video_meta(&info);
        assert_eq!(meta.lang.as_deref(), Some("de"));
    }

    #[test]
    fn select_subtitle_lang_cases() {
        let info = serde_json::json!({});
        assert!(select_subtitle_lang(&info, "en").is_none());

        let info = serde_json::json!({
            "subtitles": {},
            "automatic_captions": {},
        });
        assert!(select_subtitle_lang(&info, "en").is_none());

        let info = serde_json::json!({
            "subtitles": {"en": [{"ext": "srv3"}]},
            "automatic_captions": {"en": [{"ext": "srv3"}]},
        });

        let (lang, is_asr) = select_subtitle_lang(&info, "en").unwrap();
        assert_eq!(lang, "en");
        assert!(!is_asr);

        let info = serde_json::json!({
            "automatic_captions": {"en": [{"ext": "srv3"}]},
        });

        let (lang, is_asr) = select_subtitle_lang(&info, "en").unwrap();
        assert_eq!(lang, "en");
        assert!(is_asr);

        let info = serde_json::json!({
            "subtitles": {"en-US": [{"ext": "srv3"}]},
        });

        let (lang, is_asr) = select_subtitle_lang(&info, "en").unwrap();
        assert_eq!(lang, "en-US");
        assert!(!is_asr);

        let info = serde_json::json!({
            "automatic_captions": {"en-orig": [{"ext": "srv3"}]},
        });

        let (lang, is_asr) = select_subtitle_lang(&info, "en").unwrap();
        assert_eq!(lang, "en-orig");
        assert!(is_asr);
    }
}
