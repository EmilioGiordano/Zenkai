// Shared by the engine tests; each uses some of it.
#![allow(clippy::unwrap_used, dead_code)]

// A 1x1 transparent PNG, so an image needs no file on disk.
pub const PIXEL_PNG: [u8; 67] = [
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

// The `<xf>` elements of cellXfs as written, in order.
pub fn cell_xfs(styles: &str) -> Vec<String> {
    let document = roxmltree::Document::parse(styles).unwrap();
    let list = document
        .root_element()
        .children()
        .find(|node| node.has_tag_name("cellXfs"))
        .unwrap();
    list.children()
        .filter(roxmltree::Node::is_element)
        .map(|node| styles[node.range()].to_string())
        .collect()
}

// Every list's count attribute says how many items it holds.
pub fn assert_counts_match(styles: &str) {
    let document = roxmltree::Document::parse(styles).unwrap();
    for list in document.root_element().children().filter(|node| {
        ["numFmts", "fonts", "fills", "borders", "cellXfs"]
            .iter()
            .any(|name| node.has_tag_name(*name))
    }) {
        let items = list.children().filter(roxmltree::Node::is_element).count();
        let declared: usize = list.attribute("count").unwrap().parse().unwrap();
        assert_eq!(declared, items, "{}", list.tag_name().name());
    }
}
