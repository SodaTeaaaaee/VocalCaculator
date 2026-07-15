use std::path::Path;

use ed25519_dalek::{
    PUBLIC_KEY_LENGTH, SECRET_KEY_LENGTH, Signature, Signer, SigningKey, VerifyingKey,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Cryptographic identity for a network device.
///
/// Each device holds an Ed25519 keypair and a stable `node_id` derived
/// deterministically from the public key.  The identity persists across
/// restarts by saving raw key bytes to disk.
pub struct DeviceIdentity {
    secret_key: SigningKey,
    public_key: VerifyingKey,
    node_id: Uuid,
}

impl DeviceIdentity {
    /// Generate a brand-new random identity.
    pub fn generate() -> Self {
        let mut secret_bytes = [0u8; SECRET_KEY_LENGTH];
        getrandom::getrandom(&mut secret_bytes).expect("OS random number generator failed");
        let secret_key = SigningKey::from_bytes(&secret_bytes);
        let public_key = secret_key.verifying_key();
        let node_id = derive_node_id(&public_key);
        Self {
            secret_key,
            public_key,
            node_id,
        }
    }

    /// Load an existing identity from a directory, or create and persist a new one.
    ///
    /// Expects `identity.key` (32-byte secret key) and `identity.pub`
    /// (32-byte public key) inside `dir`.  If either file is missing a
    /// fresh identity is generated and saved.
    pub fn load_or_create(dir: &Path) -> Result<Self, anyhow::Error> {
        let key_path = dir.join("identity.key");
        let pub_path = dir.join("identity.pub");

        if key_path.exists() && pub_path.exists() {
            let secret_bytes = std::fs::read(&key_path)?;
            let public_bytes = std::fs::read(&pub_path)?;

            if secret_bytes.len() != SECRET_KEY_LENGTH {
                anyhow::bail!(
                    "identity.key has invalid length {} (expected {SECRET_KEY_LENGTH})",
                    secret_bytes.len()
                );
            }
            if public_bytes.len() != PUBLIC_KEY_LENGTH {
                anyhow::bail!(
                    "identity.pub has invalid length {} (expected {PUBLIC_KEY_LENGTH})",
                    public_bytes.len()
                );
            }

            let mut sk = [0u8; SECRET_KEY_LENGTH];
            sk.copy_from_slice(&secret_bytes);
            let secret_key = SigningKey::from_bytes(&sk);

            let mut pk = [0u8; PUBLIC_KEY_LENGTH];
            pk.copy_from_slice(&public_bytes);
            let public_key = VerifyingKey::from_bytes(&pk)?;

            // Sanity-check: the loaded public key must match the secret key.
            let derived_pub = secret_key.verifying_key();
            if derived_pub != public_key {
                anyhow::bail!("identity.key and identity.pub do not form a valid keypair");
            }

            let node_id = derive_node_id(&public_key);

            log::info!("Loaded device identity from {}", dir.display());
            return Ok(Self {
                secret_key,
                public_key,
                node_id,
            });
        }

        // No existing identity -- generate and persist.
        let identity = Self::generate();
        identity.save(dir)?;
        log::info!("Generated new device identity at {}", dir.display());
        Ok(identity)
    }

    /// Persist the identity to a directory as raw key files.
    pub fn save(&self, dir: &Path) -> Result<(), anyhow::Error> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("identity.key"), self.secret_key.to_bytes())?;
        std::fs::write(dir.join("identity.pub"), self.public_key.to_bytes())?;
        Ok(())
    }

    /// Sign a message with the device's secret key.
    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.secret_key.sign(msg)
    }

    /// Return a clone of the signing key for the network handshake.
    pub(crate) fn signing_key(&self) -> SigningKey {
        self.secret_key.clone()
    }

    /// Return the 32-byte public key.
    pub fn public_key_bytes(&self) -> [u8; PUBLIC_KEY_LENGTH] {
        self.public_key.to_bytes()
    }

    /// Return the verifying key (for external verification).
    pub fn verifying_key(&self) -> VerifyingKey {
        self.public_key
    }

    /// Return the stable node identifier derived from the public key.
    pub fn node_id(&self) -> Uuid {
        self.node_id
    }
}

/// Derive a deterministic UUID from an Ed25519 public key.
///
/// SHA256(pubkey) -> first 16 bytes -> set UUID v5 version & variant bits.
pub(crate) fn derive_node_id(public_key: &VerifyingKey) -> Uuid {
    let hash = Sha256::digest(public_key.to_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);

    // Set version 5 (0101 in bits 4-7 of byte 6).
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    // Set variant 10xx (bits 6-7 of byte 8).
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;
    use tempfile::TempDir;

    #[test]
    fn generate_produces_valid_identity() {
        let id = DeviceIdentity::generate();
        // Node ID should be non-nil.
        assert_ne!(id.node_id(), Uuid::nil());
        // Public key bytes should be 32 bytes.
        assert_eq!(id.public_key_bytes().len(), PUBLIC_KEY_LENGTH);
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let id = DeviceIdentity::generate();
        let msg = b"hello vocal calculator";
        let sig = id.sign(msg);
        assert!(id.verifying_key().verify(msg, &sig).is_ok());
        // Tampered message should fail.
        let bad = b"hello vocal calculato";
        assert!(id.verifying_key().verify(bad, &sig).is_err());
    }

    #[test]
    fn save_and_load_roundtrip() -> Result<(), anyhow::Error> {
        let dir = TempDir::new()?;
        let original = DeviceIdentity::generate();
        original.save(dir.path())?;

        let loaded = DeviceIdentity::load_or_create(dir.path())?;
        assert_eq!(original.node_id(), loaded.node_id());
        assert_eq!(original.public_key_bytes(), loaded.public_key_bytes());

        // Loaded identity should sign and verify correctly.
        let msg = b"roundtrip test";
        let sig = loaded.sign(msg);
        assert!(loaded.verifying_key().verify(msg, &sig).is_ok());
        Ok(())
    }

    #[test]
    fn load_or_create_generates_when_missing() -> Result<(), anyhow::Error> {
        let dir = TempDir::new()?;
        // First call generates.
        let id1 = DeviceIdentity::load_or_create(dir.path())?;
        // Second call loads the same identity.
        let id2 = DeviceIdentity::load_or_create(dir.path())?;
        assert_eq!(id1.node_id(), id2.node_id());
        assert_eq!(id1.public_key_bytes(), id2.public_key_bytes());
        Ok(())
    }

    #[test]
    fn node_id_is_deterministic() {
        // Two calls with the same public key must produce the same UUID.
        let id = DeviceIdentity::generate();
        let expected = derive_node_id(&id.verifying_key());
        assert_eq!(id.node_id(), expected);
    }

    #[test]
    fn corrupt_key_file_fails_gracefully() -> Result<(), anyhow::Error> {
        let dir = TempDir::new()?;
        // Write garbage to identity.key.
        std::fs::write(dir.path().join("identity.key"), b"too short")?;
        std::fs::write(dir.path().join("identity.pub"), [0u8; PUBLIC_KEY_LENGTH])?;
        assert!(DeviceIdentity::load_or_create(dir.path()).is_err());
        Ok(())
    }
}
