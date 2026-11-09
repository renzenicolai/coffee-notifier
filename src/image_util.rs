//! Decoding of the `gif` field on a [`crate::config::Rule`]: an inline, base64-encoded image
//! (a `data:` URL, as produced by the browser plugin's file upload) or a plain `http(s)://`
//! URL. Only inline `data:` images are actually decoded/displayed - URLs merely round-trip
//! through the configuration so an exported plugin config stays intact.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use base64::{engine::general_purpose::STANDARD, Engine};

pub struct DecodedImage {
    pub bytes: Vec<u8>,
    pub extension: &'static str,
}

/// Decodes a `data:` URL into raw image bytes. Returns `None` for anything else (an
/// `http(s)://` URL, an empty string, or a malformed data URL).
pub fn decode_data_url(gif: &str) -> Option<DecodedImage> {
    let rest = gif.trim().strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mime = meta.split(';').next().unwrap_or("");
    let bytes = STANDARD.decode(data).ok()?;
    Some(DecodedImage {
        bytes,
        extension: extension_for_mime(mime),
    })
}

fn extension_for_mime(mime: &str) -> &'static str {
    match mime.trim() {
        "image/gif" => "gif",
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        _ => "img",
    }
}

/// Decodes a rule's `gif` field for on-screen preview: pixel dimensions plus RGBA8 data,
/// suitable for an `egui::ColorImage`.
pub fn decode_for_preview(gif: &str) -> Option<(u32, u32, Vec<u8>)> {
    let decoded = decode_data_url(gif)?;
    let dyn_img = image::load_from_memory(&decoded.bytes).ok()?;
    let rgba = dyn_img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some((width, height, rgba.into_raw()))
}

/// Writes a rule's `gif` field out to a stable temp file (named after a hash of its bytes, so
/// identical images are reused and different ones never collide) for handing to the OS
/// notification API as an `image-path` hint. Returns `None` if there's nothing to show.
pub fn materialize_preview(gif: &str) -> Option<PathBuf> {
    let decoded = decode_data_url(gif)?;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    decoded.bytes.hash(&mut hasher);
    let path = std::env::temp_dir().join(format!("dcn-notify-img-{:016x}.{}", hasher.finish(), decoded.extension));

    if !path.exists() {
        std::fs::write(&path, &decoded.bytes).ok()?;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_data_url() {
        let data = STANDARD.encode(b"not really a gif");
        let url = format!("data:image/gif;base64,{data}");
        let decoded = decode_data_url(&url).unwrap();
        assert_eq!(decoded.bytes, b"not really a gif");
        assert_eq!(decoded.extension, "gif");
    }

    #[test]
    fn rejects_remote_urls() {
        assert!(decode_data_url("https://example.com/a.gif").is_none());
        assert!(decode_data_url("").is_none());
    }

    #[test]
    fn materialize_preview_writes_a_reusable_file() {
        let data = STANDARD.encode(b"hello image bytes");
        let url = format!("data:image/png;base64,{data}");
        let path = materialize_preview(&url).unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"hello image bytes");

        // Same content -> same path, and it's left alone the second time around.
        let path2 = materialize_preview(&url).unwrap();
        assert_eq!(path, path2);

        let _ = std::fs::remove_file(&path);
    }
}
