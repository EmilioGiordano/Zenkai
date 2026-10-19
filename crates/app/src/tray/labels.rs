use zenkai_i18n::t;

pub fn workbooks_label(count: usize) -> String {
    t!("tray.workbooks", count = count)
}

pub fn memory_label(committed_mb: u64) -> String {
    let separator = t!("tray.decimal_separator");
    t!("tray.memory", gb = gigabytes_text(committed_mb, separator))
}

fn gigabytes_text(megabytes: u64, separator: &str) -> String {
    let tenths = (megabytes * 10 + 512) / 1024;
    format!("{}{separator}{}", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_is_rounded_to_one_decimal_of_a_gigabyte() {
        assert_eq!(gigabytes_text(0, "."), "0.0");
        assert_eq!(gigabytes_text(1024, "."), "1.0");
        assert_eq!(gigabytes_text(1536, "."), "1.5");
        assert_eq!(gigabytes_text(1500, ","), "1,5");
        assert_eq!(gigabytes_text(10_240 + 50, "."), "10.0");
    }

    #[test]
    fn labels_follow_the_catalog() {
        assert_eq!(workbooks_label(1), "1 workbook open");
        assert_eq!(workbooks_label(3), "3 workbooks open");
        assert_eq!(memory_label(2048), "Memory: 2.0 GB");
    }
}
