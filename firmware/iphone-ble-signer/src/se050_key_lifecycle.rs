//! Root-key creation policy, independent of the I2C transport for fault testing.
use anyhow::Result;

pub trait RootKeyStore {
    fn exists(&mut self) -> Result<bool>;
    fn validate_public_key(&mut self) -> Result<()>;
    /// Must refuse to replace an existing object. Never delete as part of creation.
    fn create_missing_key(&mut self) -> Result<()>;
}

pub fn ensure_root_key(store: &mut impl RootKeyStore) -> Result<()> {
    // A transport/APDU/decoding error is not evidence that a key is absent.
    if !store.exists()? {
        store.create_missing_key()?;
    }
    store.validate_public_key()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    struct Store {
        presence: Option<bool>,
        readable: bool,
        creates: usize,
    }
    impl RootKeyStore for Store {
        fn exists(&mut self) -> Result<bool> {
            self.presence.ok_or_else(|| anyhow!("I2C timeout"))
        }
        fn validate_public_key(&mut self) -> Result<()> {
            anyhow::ensure!(self.readable, "public-key read failed");
            Ok(())
        }
        fn create_missing_key(&mut self) -> Result<()> {
            self.creates += 1;
            self.presence = Some(true);
            Ok(())
        }
    }
    #[test]
    fn presence_error_never_creates_a_key() {
        let mut store = Store {
            presence: None,
            readable: true,
            creates: 0,
        };
        assert!(ensure_root_key(&mut store).is_err());
        assert_eq!(store.creates, 0);
    }
    #[test]
    fn existing_unreadable_key_is_preserved() {
        let mut store = Store {
            presence: Some(true),
            readable: false,
            creates: 0,
        };
        assert!(ensure_root_key(&mut store).is_err());
        assert_eq!(store.creates, 0);
    }
    #[test]
    fn existing_key_is_only_validated() {
        let mut store = Store {
            presence: Some(true),
            readable: true,
            creates: 0,
        };
        ensure_root_key(&mut store).unwrap();
        assert_eq!(store.creates, 0);
    }
    #[test]
    fn confirmed_absence_creates_once_and_retry_preserves_it() {
        let mut store = Store {
            presence: Some(false),
            readable: false,
            creates: 0,
        };
        assert!(ensure_root_key(&mut store).is_err());
        store.readable = true;
        ensure_root_key(&mut store).unwrap();
        assert_eq!(store.creates, 1);
    }
}
