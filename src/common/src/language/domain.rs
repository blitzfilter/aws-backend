use crate::language::command::LanguageCommand;

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
