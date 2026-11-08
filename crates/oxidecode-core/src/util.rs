use rand::Rng;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Writes a file atomically: temp file in the same directory, fsync, rename,
/// then fsync the parent directory. Preserves permissions of an existing file.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let suffix: u32 = rand::thread_rng().gen();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let temp = parent.join(format!(".{name}.tmp.{suffix}"));
    let existing_mode = fs::metadata(path).ok().map(|meta| {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode()
    });

    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        if let Some(mode) = existing_mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&temp, fs::Permissions::from_mode(mode));
        }
        drop(file);
        fs::rename(&temp, path)?;
        let _ = oxidecode_sys::fsync_dir(path);
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Reads up to `max` bytes as UTF-8, replacing invalid sequences.
pub fn read_lossy_limited(path: &Path, max: usize) -> io::Result<(String, bool)> {
    let mut file = File::open(path)?;
    let mut buffer = Vec::with_capacity(max.min(64 * 1024));
    std::io::Read::by_ref(&mut file)
        .take(max as u64 + 1)
        .read_to_end(&mut buffer)?;
    let truncated = buffer.len() > max;
    buffer.truncate(max);
    Ok((String::from_utf8_lossy(&buffer).into_owned(), truncated))
}

/// Truncates tool output by lines and bytes, returning the text and a flag.
pub fn truncate_output(text: &str, max_lines: usize, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes && text.lines().count() <= max_lines {
        return (text.to_string(), false);
    }
    let mut out = String::with_capacity(max_bytes.min(text.len()));
    for (index, line) in text.lines().enumerate() {
        if index >= max_lines || out.len() + line.len() + 1 > max_bytes {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    let removed_lines = text.lines().count().saturating_sub(out.lines().count());
    let removed_bytes = text.len().saturating_sub(out.len());
    out.push_str(&format!(
        "\n[truncated: {removed_lines} lines / {removed_bytes} bytes omitted]"
    ));
    (out, true)
}

/// Expands OpenCode-style `{env:NAME}` and `{file:path}` references.
pub fn expand_vars(input: &str, base_dir: &Path, home: &Path) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = match tail.find('}') {
            Some(end) => end,
            None => {
                out.push_str(tail);
                return out;
            }
        };
        let token = &tail[1..end];
        if let Some(name) = token.strip_prefix("env:") {
            out.push_str(&std::env::var(name).unwrap_or_default());
        } else if let Some(path) = token.strip_prefix("file:") {
            let candidate = if let Some(relative) = path.strip_prefix("~/") {
                home.join(relative)
            } else if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                base_dir.join(path)
            };
            let value = fs::read_to_string(&candidate)
                .unwrap_or_default()
                .trim_end()
                .to_string();
            out.push_str(&value);
        } else {
            out.push_str(&tail[..=end]);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}
