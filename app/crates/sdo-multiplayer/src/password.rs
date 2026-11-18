//! Ephemeral room credentials are never included in room snapshots or settings.
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};

pub(super) struct RoomPassword {
    salt: [u8; 16],
    digest: [u8; 32],
}
impl RoomPassword {
    pub fn new(password: &str) -> Result<Option<Self>> {
        ensure!(
            password.len() <= crate::MAX_ROOM_PASSWORD_BYTES
                && !password.chars().any(char::is_control),
            "Room passwords must be at most 128 bytes without control characters"
        );
        if password.is_empty() {
            return Ok(None);
        }
        let mut salt = [0; 16];
        getrandom::fill(&mut salt)
            .map_err(|error| anyhow::anyhow!("Cannot create room password salt: {error}"))?;
        Ok(Some(Self {
            salt,
            digest: Self::digest(&salt, password),
        }))
    }
    fn digest(salt: &[u8; 16], password: &str) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(salt);
        hash.update(password.as_bytes());
        hash.finalize().into()
    }
    pub fn matches(&self, password: &str) -> bool {
        if password.len() > crate::MAX_ROOM_PASSWORD_BYTES {
            return false;
        }
        Self::digest(&self.salt, password)
            .iter()
            .zip(self.digest)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_passwords_are_salted_exact_and_bounded() -> Result<()> {
        assert!(RoomPassword::new("")?.is_none());
        let a = RoomPassword::new(" 密碼 ")?.unwrap();
        let b = RoomPassword::new(" 密碼 ")?.unwrap();
        assert!(a.matches(" 密碼 "));
        assert!(!a.matches("密碼"));
        assert!(!a.matches(""));
        assert_ne!(a.salt, b.salt);
        assert_ne!(a.digest, b.digest);
        assert!(RoomPassword::new(&"x".repeat(crate::MAX_ROOM_PASSWORD_BYTES + 1)).is_err());
        assert!(RoomPassword::new("bad\npassword").is_err());
        Ok(())
    }
}
