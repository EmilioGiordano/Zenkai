use markdown::mdast::Node;

use crate::markdown_links::shown_target;
use crate::markdown_restricted::{plain_text, vetted_link};
use crate::updates::Version;

pub const MAX_LINKS: usize = 12;
const MAX_TEXT_CHARS: usize = 60;
const REPOSITORY: &str = "https://github.com/EmilioGiordano/Zenkai";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Release(Version),
    Issue(u32),
    Pull(u32),
    Other,
}

// `url` is always the address `vetted_link` returned, so it is what the prompt shows and the
// browser opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteLink {
    pub url: String,
    pub target: Target,
    pub text: String,
}

pub fn release_page(version: Version) -> String {
    format!("{REPOSITORY}/releases/tag/v{version}")
}

// The release page of the version first, then the vetted links of its parsed text in the
// order they appear, without repeats. A version always has its release page.
pub fn links_of(version: Version, tree: Option<&Node>) -> Vec<NoteLink> {
    let mut links = vec![NoteLink {
        target: Target::Release(version),
        text: String::new(),
        url: release_page(version),
    }];
    let mut pending: Vec<&Node> = tree.into_iter().collect();
    while let Some(node) = pending.pop() {
        if links.len() == MAX_LINKS {
            break;
        }
        if let Node::Link(link) = node
            && let Some(url) = vetted_link(&link.url)
            && !links.iter().any(|known| known.url == url)
        {
            links.push(NoteLink {
                target: target_of(&url),
                text: cut(&plain_text(&link.children), &url),
                url,
            });
        }
        if let Some(children) = node.children() {
            pending.extend(children.iter().rev());
        }
    }
    links
}

fn cut(text: &str, url: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return shown_target(url);
    }
    if text.chars().count() <= MAX_TEXT_CHARS {
        return text;
    }
    let mut shortened: String = text.chars().take(MAX_TEXT_CHARS).collect();
    shortened.push('\u{2026}');
    shortened
}

fn number(text: &str) -> Option<u32> {
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        text.parse().ok()
    } else {
        None
    }
}

fn target_of(url: &str) -> Target {
    let Some(path) = url
        .strip_prefix(REPOSITORY)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return Target::Other;
    };
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut parts = path.trim_end_matches('/').split('/');
    let target = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("releases"), Some("tag"), Some(tag), None) => {
            Version::parse_tag(tag).map(Target::Release)
        }
        (Some("issues"), Some(found), None, None) => number(found).map(Target::Issue),
        (Some("pull"), Some(found), None, None) => number(found).map(Target::Pull),
        _ => None,
    };
    target.unwrap_or(Target::Other)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::markdown_restricted;

    fn version(text: &str) -> Version {
        Version::parse_bare(text).unwrap()
    }

    fn links_in(version: Version, text: Option<&str>) -> Vec<NoteLink> {
        let checked = text.and_then(markdown_restricted::check);
        links_of(version, checked.as_ref().map(|checked| &checked.tree))
    }

    fn urls(links: &[NoteLink]) -> Vec<&str> {
        links.iter().map(|link| link.url.as_str()).collect()
    }

    #[test]
    fn the_release_page_is_an_allowed_link() {
        let page = release_page(version("0.1.3"));
        assert_eq!(vetted_link(&page).as_deref(), Some(page.as_str()));
        assert_eq!(
            page,
            "https://github.com/EmilioGiordano/Zenkai/releases/tag/v0.1.3"
        );
    }

    #[test]
    fn a_version_without_text_or_links_has_only_its_release_page() {
        for text in [None, Some(""), Some("- fixed a bug\n- and another\n")] {
            let links = links_in(version("0.1.3"), text);
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].target, Target::Release(version("0.1.3")));
        }
    }

    #[test]
    fn links_come_after_the_release_page_in_order_without_repeats() {
        let text = "See [#12](https://github.com/EmilioGiordano/Zenkai/issues/12) and \
                    [the fix](https://github.com/EmilioGiordano/Zenkai/pull/13), also \
                    [again](https://github.com/EmilioGiordano/Zenkai/issues/12) and \
                    [this release](https://github.com/EmilioGiordano/Zenkai/releases/tag/v0.1.3).";
        let links = links_in(version("0.1.3"), Some(text));
        assert_eq!(
            urls(&links),
            [
                "https://github.com/EmilioGiordano/Zenkai/releases/tag/v0.1.3",
                "https://github.com/EmilioGiordano/Zenkai/issues/12",
                "https://github.com/EmilioGiordano/Zenkai/pull/13",
            ]
        );
        assert_eq!(links[1].target, Target::Issue(12));
        assert_eq!(links[1].text, "#12");
        assert_eq!(links[2].target, Target::Pull(13));
    }

    #[test]
    fn links_that_are_not_allowed_are_left_out() {
        let text = "[a](https://evil.example/EmilioGiordano/Zenkai) \
                    [b](http://github.com/EmilioGiordano/Zenkai/issues/1) \
                    [c](file:///C:/x) [d](#top) [e](https://github.com/Other/Zenkai) \
                    [f](https://github.com/EmilioGiordano/Zenkai/../Other)";
        assert_eq!(links_in(version("0.1.3"), Some(text)).len(), 1);
    }

    #[test]
    fn other_links_are_named_by_their_text_or_else_their_address() {
        let text = "[the wiki](https://github.com/EmilioGiordano/Zenkai/wiki) \
                    [](https://github.com/EmilioGiordano/Zenkai/discussions)";
        let links = links_in(version("0.1.3"), Some(text));
        assert_eq!(links[1].target, Target::Other);
        assert_eq!(links[1].text, "the wiki");
        assert_eq!(
            links[2].text,
            "https://github.com/EmilioGiordano/Zenkai/discussions"
        );
    }

    #[test]
    fn a_long_text_is_cut_and_marked() {
        let text = format!(
            "[{}](https://github.com/EmilioGiordano/Zenkai/wiki)",
            "word ".repeat(40)
        );
        let links = links_in(version("0.1.3"), Some(&text));
        assert_eq!(links[1].text.chars().count(), MAX_TEXT_CHARS + 1);
        assert!(links[1].text.ends_with('\u{2026}'));
    }

    #[test]
    fn bare_addresses_and_angle_links_count_and_code_does_not() {
        let text = "https://github.com/EmilioGiordano/Zenkai/issues/5 and \
                    <https://github.com/EmilioGiordano/Zenkai/pull/6>\n\n\
                    `[x](https://github.com/EmilioGiordano/Zenkai/issues/7)`\n\n\
                    ```\nhttps://github.com/EmilioGiordano/Zenkai/issues/8\n```\n";
        let links = links_in(version("0.1.3"), Some(text));
        assert_eq!(
            urls(&links)[1..],
            [
                "https://github.com/EmilioGiordano/Zenkai/issues/5",
                "https://github.com/EmilioGiordano/Zenkai/pull/6",
            ]
        );
    }

    #[test]
    fn a_reference_link_is_not_a_link() {
        let text = "[a][ref]\n\n[ref]: https://github.com/EmilioGiordano/Zenkai/issues/9\n";
        assert_eq!(links_in(version("0.1.3"), Some(text)).len(), 1);
    }

    #[test]
    fn no_more_than_the_limit_are_listed() {
        let text: String = (1..=50)
            .map(|number| {
                format!("[{number}](https://github.com/EmilioGiordano/Zenkai/issues/{number}) ")
            })
            .collect();
        assert_eq!(links_in(version("0.1.3"), Some(&text)).len(), MAX_LINKS);
    }

    #[test]
    fn targets_are_read_from_the_address() {
        let of = |path: &str| target_of(&format!("{REPOSITORY}{path}"));
        assert_eq!(
            of("/releases/tag/v0.2.0"),
            Target::Release(version("0.2.0"))
        );
        assert_eq!(of("/releases/tag/nightly"), Target::Other);
        assert_eq!(of("/releases"), Target::Other);
        assert_eq!(of("/issues/301"), Target::Issue(301));
        assert_eq!(of("/issues/301?x=1#c"), Target::Issue(301));
        assert_eq!(of("/issues/+301"), Target::Other);
        assert_eq!(of("/issues/99999999999"), Target::Other);
        assert_eq!(of("/issues/3/files"), Target::Other);
        assert_eq!(of("/pull/7/"), Target::Pull(7));
        assert_eq!(target_of(REPOSITORY), Target::Other);
    }

    const PIECES: &[&str] = &[
        "[",
        "](",
        ")",
        "](https://github.com/EmilioGiordano/Zenkai/issues/1)",
        "https://github.com/EmilioGiordano/Zenkai/pull/2",
        "<https://github.com/EmilioGiordano/Zenkai/>",
        "https://evil.example/",
        "[ref]: https://github.com/EmilioGiordano/Zenkai/\n",
        "![img](https://github.com/EmilioGiordano/Zenkai/a.png)",
        "<a href=\"https://github.com/EmilioGiordano/Zenkai/\">",
        "`",
        "```\n",
        "\n",
        "> ",
        "- ",
        "*",
        "text ",
    ];

    proptest! {
        #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

        #[test]
        fn whatever_the_text_the_links_are_allowed_and_few(
            parts in prop::collection::vec(prop::sample::select(PIECES.to_vec()), 0..30)
        ) {
            let text = parts.concat();
            let links = links_in(version("0.1.3"), Some(&text));
            prop_assert!(!links.is_empty() && links.len() <= MAX_LINKS);
            prop_assert_eq!(&links[0].url, &release_page(version("0.1.3")));
            for link in &links {
                prop_assert_eq!(vetted_link(&link.url).as_deref(), Some(link.url.as_str()));
            }
            let mut seen = std::collections::HashSet::new();
            prop_assert!(links.iter().all(|link| seen.insert(&link.url)));
        }

        #[test]
        fn any_characters_are_read_without_a_panic(
            text in prop::collection::vec(any::<char>(), 0..200).prop_map(String::from_iter)
        ) {
            let links = links_in(version("0.1.3"), Some(&text));
            prop_assert!(links.len() <= MAX_LINKS);
        }
    }
}
