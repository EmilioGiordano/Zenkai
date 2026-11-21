use std::io;

use winsafe::{HWND, co, prelude::*};

const RELEASES: &str = "https://github.com/EmilioGiordano/Zenkai/releases";

// What the launcher says itself, in a message box, when no app is there to say it.
#[derive(Debug)]
pub enum Message {
    Busy,
    Reinstall,
    Interrupted,
    NotClosed,
    CouldNotStart(io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Language {
    English,
    Spanish,
}

impl Message {
    fn text(&self, language: Language) -> String {
        match (self, language) {
            (Message::Busy, Language::English) => {
                "Zenkai is finishing an update. Try again in a moment.".to_owned()
            }
            (Message::Busy, Language::Spanish) => {
                "Zenkai está terminando una actualización. Vuelve a intentarlo en un momento."
                    .to_owned()
            }
            (Message::Reinstall, Language::English) => {
                format!("Zenkai's program files are missing. Download it again from {RELEASES}")
            }
            (Message::Reinstall, Language::Spanish) => {
                format!("Faltan los archivos de Zenkai. Descárgalo de nuevo desde {RELEASES}")
            }
            (Message::Interrupted, Language::English) => {
                "An update of Zenkai could not finish. Open Zenkai again to restore the previous version."
                    .to_owned()
            }
            (Message::Interrupted, Language::Spanish) => {
                "Una actualización de Zenkai no pudo terminar. Abre Zenkai de nuevo para restaurar la versión anterior."
                    .to_owned()
            }
            (Message::NotClosed, Language::English) => {
                "Zenkai did not close in time, so the update was not installed.".to_owned()
            }
            (Message::NotClosed, Language::Spanish) => {
                "Zenkai no se cerró a tiempo, así que la actualización no se instaló.".to_owned()
            }
            (Message::CouldNotStart(error), Language::English) => {
                format!("Zenkai could not start: {error}")
            }
            (Message::CouldNotStart(error), Language::Spanish) => {
                format!("Zenkai no pudo iniciarse: {error}")
            }
        }
    }
}

pub fn show(message: &Message) {
    tracing::warn!(?message, "telling the user");
    let language = match sys_locale::get_locale() {
        Some(tag) if tag.starts_with("es") => Language::Spanish,
        _ => Language::English,
    };
    let shown = HWND::NULL.MessageBox(
        &message.text(language),
        "Zenkai",
        co::MB::OK | co::MB::ICONWARNING,
    );
    if let Err(error) = shown {
        tracing::error!(%error, "could not show the message");
    }
}

#[cfg(test)]
mod tests {
    use super::{Language, Message};

    #[test]
    fn every_message_has_both_languages() {
        let messages = [
            Message::Busy,
            Message::Reinstall,
            Message::Interrupted,
            Message::NotClosed,
            Message::CouldNotStart(std::io::Error::other("boom")),
        ];
        for message in &messages {
            let english = message.text(Language::English);
            let spanish = message.text(Language::Spanish);
            assert!(!english.is_empty() && !spanish.is_empty());
            assert_ne!(english, spanish, "{message:?}");
        }
    }
}
