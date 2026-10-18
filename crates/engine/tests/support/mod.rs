// Shared by the tests that read a saved file's styles part; each uses some of it.
#![allow(clippy::unwrap_used, dead_code)]

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
