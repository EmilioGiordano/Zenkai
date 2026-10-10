use crate::tools::approval_text::shown_text;

const VERSION_CHARS: usize = 16;

const KNOWN: [(&str, &str); 4] = [
    ("claude-code", "Claude Code"),
    ("gemini", "Gemini CLI"),
    ("codex", "Codex"),
    ("opencode", "OpenCode"),
];

// The client names itself in the MCP handshake, so the text is untrusted: control and
// format characters are escaped and the length is capped. Known clients get their usual name.
pub fn client_label(name: &str, version: &str) -> Option<String> {
    let lowered = name.to_lowercase().replace([' ', '_'], "-");
    let friendly = KNOWN
        .iter()
        .find(|(key, _)| lowered.contains(key))
        .map(|(_, friendly)| (*friendly).to_string());
    let shown_name = friendly.unwrap_or_else(|| shown_text(name.trim()));
    if shown_name.trim().is_empty() {
        return None;
    }
    let version: String = version.trim().chars().take(VERSION_CHARS).collect();
    let version = shown_text(&version);
    if version.is_empty() {
        Some(shown_name)
    } else {
        Some(format!("{shown_name} {version}"))
    }
}

#[cfg(test)]
mod tests {
    use super::client_label;

    #[test]
    fn known_clients_get_their_usual_name() {
        assert_eq!(
            client_label("claude-code", "2.1.4").as_deref(),
            Some("Claude Code 2.1.4")
        );
        assert_eq!(
            client_label("gemini-cli-mcp-client", "0.3").as_deref(),
            Some("Gemini CLI 0.3")
        );
        assert_eq!(client_label("codex", "").as_deref(), Some("Codex"));
        assert_eq!(
            client_label("opencode", "1.18.32").as_deref(),
            Some("OpenCode 1.18.32")
        );
    }

    #[test]
    fn unknown_names_are_escaped_and_capped() {
        let label = client_label("evil\u{202E}name\nline", "1").unwrap();
        assert!(
            label.contains("\\u{202E}") && label.contains("\\n"),
            "{label}"
        );
        let long = client_label(&"x".repeat(500), "1").unwrap();
        assert!(long.chars().count() < 70, "{long}");
    }

    #[test]
    fn an_empty_name_means_no_client() {
        assert_eq!(client_label("  ", "1"), None);
    }
}
