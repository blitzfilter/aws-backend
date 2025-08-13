pub trait HasKey {
    type Key;
    fn key(&self) -> Self::Key;

    fn key_string(&self) -> String
    where
        Self::Key: Into<String>,
    {
        self.key().into()
    }

    fn key_from_string(string: String) -> Self::Key
    where
        Self::Key: From<String>,
    {
        Self::Key::from(string)
    }
}
