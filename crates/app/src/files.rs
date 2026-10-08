use std::path::{Path, PathBuf};

const REPLACED_EXTENSIONS: [&str; 6] = ["xlsm", "xls", "xlsb", "ods", "csv", "tsv"];

pub fn xlsx_target(chosen: &Path) -> PathBuf {
    let extension = chosen
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        Some("xlsx") => chosen.to_path_buf(),
        Some(ext) if REPLACED_EXTENSIONS.contains(&ext) => chosen.with_extension("xlsx"),
        _ => {
            let mut name = chosen.as_os_str().to_owned();
            name.push(".xlsx");
            PathBuf::from(name)
        }
    }
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_xlsx_and_never_drops_a_dotted_name() {
        assert_eq!(
            xlsx_target(Path::new("a/book.xlsx")),
            Path::new("a/book.xlsx")
        );
        assert_eq!(
            xlsx_target(Path::new("a/book.XLSM")),
            Path::new("a/book.xlsx")
        );
        assert_eq!(
            xlsx_target(Path::new("a/report.v2")),
            Path::new("a/report.v2.xlsx")
        );
        assert_eq!(xlsx_target(Path::new("a/book")), Path::new("a/book.xlsx"));
    }

    #[test]
    fn same_file_sees_through_different_spellings() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Book.xlsx");
        std::fs::write(&file, b"x").unwrap();
        let other = dir.path().join(".").join("Book.xlsx");
        assert!(same_file(&file, &other));
        assert!(!same_file(&file, &dir.path().join("Other.xlsx")));
    }
}
