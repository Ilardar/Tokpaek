//! UI language is a persisted preference; provider data stays language-neutral.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    Russian,
    English,
}

impl Language {
    pub fn text<'a>(self, russian: &'a str, english: &'a str) -> &'a str {
        match self {
            Self::Russian => russian,
            Self::English => english,
        }
    }
}

/// Keep format strings literal so both translations are checked by the compiler.
macro_rules! tr_format {
    ($lang:expr, $ru:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        match $lang {
            crate::i18n::Language::Russian => format!($ru $(, $arg)*),
            crate::i18n::Language::English => format!($en $(, $arg)*),
        }
    };
}
pub(crate) use tr_format;
