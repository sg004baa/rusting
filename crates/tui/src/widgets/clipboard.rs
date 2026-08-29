//! Lazy, persistent system clipboard ownership.

/// Owns the system clipboard after the first write so platform backends can
/// finish serving the copied contents.
#[derive(Default)]
pub(crate) struct Clipboard {
    inner: Option<arboard::Clipboard>,
}

impl Clipboard {
    pub(crate) fn set_text(&mut self, text: String) -> Result<(), arboard::Error> {
        if let Some(clipboard) = &mut self.inner {
            return clipboard.set_text(text);
        }

        let mut clipboard = arboard::Clipboard::new()?;
        let result = clipboard.set_text(text);
        self.inner = Some(clipboard);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_does_not_initialize_the_system_clipboard() {
        let clipboard = Clipboard::default();

        assert!(clipboard.inner.is_none());
    }
}
