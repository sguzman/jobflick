use anyhow::{bail, Result};
use std::io::{BufRead, Read};

// Escape-heavy JSON can require six bytes per input byte. Explicit
// Execarium YAML is bounded to 256 KiB (raw Fish remains capped at 128 KiB).
// Keep the IPC envelope large enough for a valid full YAML document.
pub const MAX_REQUEST_FRAME: usize = execarium::MAX_DOCUMENT_BYTES * 6 + 4096;
// Long job history may legitimately produce a large list, but responses
// must still be bounded to keep a broken daemon from exhausting the HUD.
pub const MAX_RESPONSE_FRAME: usize = 32 * 1024 * 1024;

/// Read a single newline-delimited JSON frame with an enforced byte limit.
/// A missing terminator, even at EOF, is not a complete request/response.
pub fn read_frame<R: BufRead>(reader: R, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit.saturating_add(1) as u64).read_until(b'\n', &mut bytes)?;
    if bytes.len() > limit {
        bail!("IPC message exceeds {} bytes", limit);
    }
    if bytes.last() != Some(&b'\n') {
        bail!("Incomplete IPC message (missing newline)");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn accepts_complete_frame() {
        assert_eq!(read_frame(Cursor::new(b"{}\n".as_slice()), 4).unwrap(), b"{}\n");
    }

    #[test]
    fn refuses_oversized_frame_before_reading_rest() {
        let content = format!("{}\n", "x".repeat(1024));
        assert!(read_frame(Cursor::new(content.as_bytes()), 16).is_err());
    }

    #[test]
    fn refuses_unterminated_frame() {
        assert!(read_frame(Cursor::new(b"{}"), 16).is_err());
    }

    #[test]
    fn permits_frame_at_limit_including_terminator() {
        assert_eq!(read_frame(Cursor::new(b"ok\n"), 3).unwrap(), b"ok\n");
    }
}
