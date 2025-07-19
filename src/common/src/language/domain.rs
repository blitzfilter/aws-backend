use crate::language::command::LanguageCommand;
use crate::language::record::LanguageRecord;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum Language {
    De,
    En,
    Fr,
    Es,
}

impl From<LanguageCommand> for Language {
    fn from(cmd: LanguageCommand) -> Self {
        match cmd {
            LanguageCommand::De => Language::De,
            LanguageCommand::En => Language::En,
            LanguageCommand::Fr => Language::Fr,
            LanguageCommand::Es => Language::Es,
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
