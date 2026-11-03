use std::fs::{self, File};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use chrono::Local;
use percent_encoding::percent_decode_str;
use wl_clipboard_rs::paste::{get_contents, get_mime_types_ordered, ClipboardType, Error, MimeType, Seat};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConflictChoice {
    Overwrite,
    Rename,
    Skip,
}

struct ConflictResolver {
    apply_all: Option<ConflictChoice>,
    is_interactive: bool,
}

impl ConflictResolver {
    fn new() -> Self {
        // When Yazi runs a shell command with `--block`, stdin is an interactive TTY.
        // Without `--block`, stdin is non-interactive (/dev/null or piped), so we
        // safely auto-rename instead of hanging or corrupting Yazi's TUI.
        let is_interactive = io::stdin().is_terminal();
        Self {
            apply_all: None,
            is_interactive,
        }
    }

    fn resolve(&mut self, target_name: &str) -> ConflictChoice {
        if let Some(choice) = self.apply_all {
            return choice;
        }

        if !self.is_interactive {
            // Non-interactive fallback: safely rename to avoid overwriting or blocking
            return ConflictChoice::Rename;
        }

        // Interactive prompt for terminal / yazi --block sessions
        print!(
            "Conflict: '{}' already exists.\n  [o]verwrite, [r]ename, [s]kip (O/R/S for all) [r]: ",
            target_name
        );
        let _ = io::stdout().flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return ConflictChoice::Rename;
        }

        let trimmed = input.trim();
        let (ch, is_upper) = match trimmed.chars().next() {
            Some(c) => (c.to_ascii_lowercase(), c.is_ascii_uppercase()),
            None => ('r', false), // Pressing Enter defaults to safe rename
        };

        let choice = match ch {
            'o' => ConflictChoice::Overwrite,
            's' => ConflictChoice::Skip,
            _ => ConflictChoice::Rename,
        };

        if is_upper {
            self.apply_all = Some(choice);
        }

        choice
    }
}

fn main() -> io::Result<()> {
    // 1. Query the offered MIME types in protocol order.
    // This probes the Wayland compositor once and avoids trial-and-error fallback attempts.
    let mime_types = match get_mime_types_ordered(ClipboardType::Regular, Seat::Unspecified) {
        Ok(m) => m,
        Err(Error::ClipboardEmpty | Error::NoSeats | Error::NoMimeType) => {
            println!("No recognized files or data in clipboard.");
            return Ok(());
        }
        Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e)),
    };

    let mut resolver = ConflictResolver::new();

    // Priority 1: Files and documents (text/uri-list)
    if mime_types.iter().any(|m| m == "text/uri-list") {
        if let Ok((mut pipe, _)) = get_contents(
            ClipboardType::Regular,
            Seat::Unspecified,
            MimeType::Specific("text/uri-list"),
        ) {
            let mut uri_list = String::new();
            if pipe.read_to_string(&mut uri_list).is_ok() {
                let mut copied_count = 0;
                let mut skipped_count = 0;

                for line in uri_list.lines() {
                    let line = line.trim();
                    if let Some(uri) = line.strip_prefix("file://") {
                        let path_part = if let Some(stripped) = uri.strip_prefix("localhost") {
                            stripped
                        } else {
                            uri
                        };

                        #[cfg(unix)]
                        let decoded_bytes: Vec<u8> = percent_decode_str(path_part).collect();
                        #[cfg(unix)]
                        let src_path = Path::new(std::ffi::OsStr::from_bytes(&decoded_bytes));

                        #[cfg(not(unix))]
                        let decoded_str = percent_decode_str(path_part).decode_utf8_lossy();
                        #[cfg(not(unix))]
                        let src_path = Path::new(&*decoded_str);

                        if src_path.symlink_metadata().is_ok() {
                            if let Some(name) = src_path.file_name() {
                                let dest_path = Path::new(".").join(name);
                                let name_str = name.to_string_lossy();

                                let (final_dest, should_copy) = if dest_path.symlink_metadata().is_ok() {
                                    match resolver.resolve(&name_str) {
                                        ConflictChoice::Skip => {
                                            println!("Skipped: '{}'", name_str);
                                            skipped_count += 1;
                                            (dest_path, false)
                                        }
                                        ConflictChoice::Rename => {
                                            let unique = get_unique_path(Path::new("."), name);
                                            let unique_name = unique
                                                .file_name()
                                                .unwrap_or(unique.as_os_str())
                                                .to_string_lossy();
                                            println!("Renamed: '{}' -> '{}'", name_str, unique_name);
                                            (unique, true)
                                        }
                                        ConflictChoice::Overwrite => {
                                            println!("Overwriting: '{}'", name_str);
                                            // Handle file-vs-dir mismatch before overwriting
                                            if dest_path.is_dir() && !src_path.is_dir() {
                                                let _ = fs::remove_dir_all(&dest_path);
                                            } else if dest_path.is_file() && src_path.is_dir() {
                                                let _ = fs::remove_file(&dest_path);
                                            }
                                            (dest_path, true)
                                        }
                                    }
                                } else {
                                    (dest_path, true)
                                };

                                if should_copy {
                                    if src_path.is_dir() {
                                        // Native recursive directory copy using Linux copy_file_range / reflink
                                        if copy_dir_all(src_path, &final_dest).is_ok() {
                                            copied_count += 1;
                                        }
                                    } else {
                                        // Native kernel copy_file_range
                                        if fs::copy(src_path, &final_dest).is_ok() {
                                            copied_count += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                if copied_count > 0 || skipped_count > 0 {
                    println!("Paste completed: {} copied, {} skipped.", copied_count, skipped_count);
                    return Ok(());
                }
            }
        }
    }

    // Priority 2: Stream image data directly from Wayland socket
    if let Some(img_mime) = find_best_image_mime(&mime_types) {
        if let Ok((mut pipe, mime_str)) = get_contents(
            ClipboardType::Regular,
            Seat::Unspecified,
            MimeType::Specific(img_mime),
        ) {
            let ext = image_mime_to_ext(&mime_str);
            let filename = generate_unique_filename(ext);
            let mut file = File::create(&filename)?;
            // io::copy directly triggers zero-copy Linux splice(2)
            io::copy(&mut pipe, &mut file)?;
            println!("Saved clipboard image as '{}'", filename);
            return Ok(());
        }
    }

    // Priority 3: Fallback to plain text streaming
    if mime_types.iter().any(|m| wl_clipboard_rs::utils::is_text(m)) {
        if let Ok((mut pipe, _)) = get_contents(
            ClipboardType::Regular,
            Seat::Unspecified,
            MimeType::Text,
        ) {
            let filename = generate_unique_filename("txt");
            let mut file = File::create(&filename)?;
            io::copy(&mut pipe, &mut file)?;
            println!("Saved clipboard text as '{}'", filename);
            return Ok(());
        }
    }

    println!("No recognized files or data in clipboard.");
    Ok(())
}

/// Recursively copies a directory without spawning external commands.
/// Uses `fs::copy` which utilizes `copy_file_range(2)` on Linux for kernel zero-copy or reflink.
fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest_path)?;
        } else if ty.is_symlink() {
            let link_target = fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(link_target, &dest_path)?;
        } else {
            fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

/// Finds a unique non-colliding path in `parent` using standard file manager conventions:
/// e.g. "file (1).ext", "file (2).ext", "folder (1)", etc.
fn get_unique_path(parent: &Path, original_name: &std::ffi::OsStr) -> PathBuf {
    let name_str = original_name.to_string_lossy();
    let (mut base, ext) = split_name(&name_str);
    let mut i = 1;

    // If the base name already ends with " (N)", continue incrementing from N + 1
    if let Some(open_idx) = base.rfind(" (") {
        if base.ends_with(')') {
            let num_str = &base[open_idx + 2..base.len() - 1];
            if let Ok(n) = num_str.parse::<usize>() {
                base = &base[..open_idx];
                i = n + 1;
            }
        }
    }

    loop {
        let candidate_name = format!("{} ({}){}", base, i, ext);
        let candidate_path = parent.join(&candidate_name);
        if candidate_path.symlink_metadata().is_err() {
            return candidate_path;
        }
        i += 1;
    }
}

/// Splits a filename into base and extension, handling compound extensions like .tar.gz
fn split_name(name: &str) -> (&str, &str) {
    for compound in &[".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"] {
        if name.ends_with(compound) && name.len() > compound.len() {
            let base_len = name.len() - compound.len();
            return (&name[..base_len], compound);
        }
    }
    if name.starts_with('.') && !name[1..].contains('.') {
        return (name, "");
    }
    if let Some(dot_idx) = name.rfind('.') {
        if dot_idx > 0 {
            return (&name[..dot_idx], &name[dot_idx..]);
        }
    }
    (name, "")
}

/// Selects the best image MIME type offered by the clipboard, preferring common lossless formats.
fn find_best_image_mime(mimes: &[String]) -> Option<&str> {
    const PREFERRED_IMAGES: &[&str] = &[
        "image/png",
        "image/jpeg",
        "image/jpg",
        "image/webp",
        "image/gif",
        "image/svg+xml",
        "image/avif",
        "image/bmp",
        "image/x-ms-bmp",
    ];
    for &preferred in PREFERRED_IMAGES {
        if let Some(m) = mimes.iter().find(|m| m.as_str() == preferred) {
            return Some(m.as_str());
        }
    }
    mimes.iter().find(|m| m.starts_with("image/")).map(|m| m.as_str())
}

/// Maps image MIME types to clean file extensions.
fn image_mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/bmp" | "image/x-ms-bmp" => "bmp",
        "image/avif" => "avif",
        "image/tiff" => "tiff",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        "image/heic" => "heic",
        "image/jxl" => "jxl",
        other => {
            if let Some(sub) = other.strip_prefix("image/") {
                let sub = sub.split('+').next().unwrap_or(sub);
                match sub {
                    "jpeg" => "jpg",
                    "svg" => "svg",
                    "webp" => "webp",
                    "png" => "png",
                    s if s.chars().all(|c| c.is_ascii_alphanumeric()) && !s.is_empty() => "png",
                    _ => "png",
                }
            } else {
                "png"
            }
        }
    }
}

/// Generates a timestamped filename with collision avoidance.
fn generate_unique_filename(ext: &str) -> String {
    let now = Local::now();
    let base = format!("clipboard_{}", now.format("%Y%m%d_%H%M%S"));
    let mut filename = format!("{}.{}", base, ext);
    let mut counter = 1;
    while Path::new(&filename).symlink_metadata().is_ok() {
        filename = format!("{} ({}).{}", base, counter, ext);
        counter += 1;
    }
    filename
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_name() {
        assert_eq!(split_name("foo.txt"), ("foo", ".txt"));
        assert_eq!(split_name("archive.tar.gz"), ("archive", ".tar.gz"));
        assert_eq!(split_name(".gitignore"), (".gitignore", ""));
        assert_eq!(split_name("folder"), ("folder", ""));
        assert_eq!(split_name("foo.bar.baz"), ("foo.bar", ".baz"));
    }

    #[test]
    fn test_get_unique_path() {
        let dir = std::env::temp_dir();
        let path = get_unique_path(&dir, std::ffi::OsStr::new("test_file_unique_xyz123.txt"));
        assert!(path.to_string_lossy().contains("test_file_unique_xyz123 (1).txt"));
    }
}
