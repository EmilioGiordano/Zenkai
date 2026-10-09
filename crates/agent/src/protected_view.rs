use std::path::Path;

// Windows URL security zones, as written by browsers and mail clients in the
// Zone.Identifier stream of a downloaded file.
const ZONE_INTERNET: u8 = 3;
const ZONE_RESTRICTED: u8 = 4;
// A real stream is a few lines; anything longer is not one Zenkai trusts.
const MAX_STREAM_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOrigin {
    Local,
    Internet,
}

// A stream that exists but names no zone fails closed, as if the file were downloaded.
pub fn parse_zone_identifier(text: &str) -> FileOrigin {
    let zone = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("ZoneId="))
        .find_map(|value| value.trim().parse::<u8>().ok());
    match zone {
        Some(ZONE_INTERNET | ZONE_RESTRICTED) | None => FileOrigin::Internet,
        Some(_) => FileOrigin::Local,
    }
}

// The Mark of the Web lives in an NTFS alternate data stream, readable with plain file
// APIs as "path:Zone.Identifier". A missing stream means a local file; one that exists
// but cannot be read is treated as a download.
pub fn file_origin(path: &Path) -> FileOrigin {
    if !cfg!(windows) {
        return FileOrigin::Local;
    }
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    let mut bytes = Vec::new();
    let read = std::fs::File::open(&stream).and_then(|file| {
        std::io::Read::read_to_end(
            &mut std::io::Read::take(file, MAX_STREAM_BYTES + 1),
            &mut bytes,
        )
    });
    match read {
        Ok(_) if bytes.len() as u64 > MAX_STREAM_BYTES => FileOrigin::Internet,
        Ok(_) => parse_zone_identifier(&String::from_utf8_lossy(&bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => FileOrigin::Local,
        Err(error) => {
            tracing::warn!(%error, "could not read the Mark of the Web; treating the file as downloaded");
            FileOrigin::Internet
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internet_and_restricted_zones_need_protected_view() {
        let browser = "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://example.com/a.xlsx\r\n";
        assert_eq!(parse_zone_identifier(browser), FileOrigin::Internet);
        assert_eq!(
            parse_zone_identifier("[ZoneTransfer]\nZoneId=4\n"),
            FileOrigin::Internet
        );
        for local in ["[ZoneTransfer]\nZoneId=2\n", "ZoneId=0", "ZoneId=1"] {
            assert_eq!(parse_zone_identifier(local), FileOrigin::Local, "{local}");
        }
        for unreadable in ["", "ZoneId=x", "[ZoneTransfer]"] {
            assert_eq!(
                parse_zone_identifier(unreadable),
                FileOrigin::Internet,
                "{unreadable}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_downloaded_file_is_recognised_from_its_stream() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("book.xlsx");
        std::fs::write(&file, b"data").unwrap();
        assert_eq!(file_origin(&file), FileOrigin::Local);
        let mut stream = file.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        std::fs::write(&stream, "[ZoneTransfer]\r\nZoneId=3\r\n").unwrap();
        assert_eq!(file_origin(&file), FileOrigin::Internet);
        assert_eq!(std::fs::read(&file).unwrap(), b"data");
        let oversized = format!("[ZoneTransfer]\r\nZoneId=2\r\n{}", " ".repeat(70_000));
        std::fs::write(&stream, oversized).unwrap();
        assert_eq!(file_origin(&file), FileOrigin::Internet);
    }
}
