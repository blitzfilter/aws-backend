use crate::language::command_data::LanguageCommandData;
use crate::language::record::LanguageRecord;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum Language {
    De,
    En,
    Fr,
    Es,
}

impl From<LanguageCommandData> for Language {
    fn from(cmd: LanguageCommandData) -> Self {
        match cmd {
            LanguageCommandData::De => Language::De,
            LanguageCommandData::En => Language::En,
            LanguageCommandData::Fr => Language::Fr,
            LanguageCommandData::Es => Language::Es,
        }
    }
}

impl From<LanguageRecord> for Language {
    fn from(cmd: LanguageRecord) -> Self {
        match cmd {
            LanguageRecord::De => Language::De,
            LanguageRecord::En => Language::En,
            LanguageRecord::Fr => Language::Fr,
            LanguageRecord::Es => Language::Es,
        }
    }
}
