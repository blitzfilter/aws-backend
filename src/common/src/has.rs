pub trait Has<T> {
    fn get(&self) -> &T;
}

impl<T> Has<T> for T {
    fn get(&self) -> &T {
        self
    }
}
