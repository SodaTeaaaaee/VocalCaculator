use std::path::Path;

use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::app::config::AppConfig;
use crate::app::identity::DeviceIdentity;

// ---------------------------------------------------------------------------
// Schema constants
// ---------------------------------------------------------------------------

const SCHEMA_VERSION: u32 = 2;

/// Metadata table: string keys to byte values.  Used for schema versioning
/// and other small configuration values.
const METADATA: TableDefinition<&str, &[u8]> = TableDefinition::new("metadata");

/// Paired devices table: 16-byte UUID keys to bincode-serialised
/// [`PairedDevice`] values.
const PAIRED_DEVICES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("paired_devices");

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// User-level trust policy for a paired device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DeviceTrust {
    /// Route requests from this device may be granted automatically.
    Trusted,
    /// Route requests from this device should ask the user each time.
    #[default]
    AskEachTime,
    /// Route requests from this device are denied.
    Blocked,
}

/// Information about a remote device that has been paired with this instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    /// Stable node identifier of the remote device.
    pub node_id: Uuid,
    /// Human-readable display name advertised during handshake.
    pub display_name: String,
    /// Last known network address (ip:port).
    pub address: String,
    /// Ed25519 public key of the remote device (32 bytes).
    pub public_key: [u8; 32],
    /// Legacy mirror of [`trust_state`](Self::trust_state) for old callers.
    pub is_trusted: bool,
    /// Epoch millis when the device was first seen.
    pub first_seen: u64,
    /// Epoch millis when the device was last seen.
    pub last_seen: u64,
    /// Epoch millis when the pairing was confirmed (user accepted).
    pub paired_at: u64,
    /// Explicit trust policy used by route authorization.
    pub trust_state: DeviceTrust,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairedDeviceV1 {
    node_id: Uuid,
    display_name: String,
    address: String,
    public_key: [u8; 32],
    is_trusted: bool,
    first_seen: u64,
    last_seen: u64,
    paired_at: u64,
}

impl From<PairedDeviceV1> for PairedDevice {
    fn from(value: PairedDeviceV1) -> Self {
        Self {
            node_id: value.node_id,
            display_name: value.display_name,
            address: value.address,
            public_key: value.public_key,
            is_trusted: value.is_trusted,
            first_seen: value.first_seen,
            last_seen: value.last_seen,
            paired_at: value.paired_at,
            trust_state: if value.is_trusted {
                DeviceTrust::Trusted
            } else {
                DeviceTrust::AskEachTime
            },
        }
    }
}

fn decode_paired_device(bytes: &[u8]) -> Result<PairedDevice, anyhow::Error> {
    match bincode::serde::decode_from_slice::<PairedDevice, _>(bytes, bincode::config::standard()) {
        Ok((device, consumed)) if consumed == bytes.len() => Ok(device),
        Ok((_device, _consumed)) => {
            let (old, _): (PairedDeviceV1, _) =
                bincode::serde::decode_from_slice(bytes, bincode::config::standard())?;
            Ok(old.into())
        }
        Err(new_err) => match bincode::serde::decode_from_slice::<PairedDeviceV1, _>(
            bytes,
            bincode::config::standard(),
        ) {
            Ok((old, _)) => Ok(old.into()),
            Err(_) => Err(new_err.into()),
        },
    }
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// Persistent storage layer backed by a local [`redb`] database.
///
/// Holds the application configuration, the local device identity, and all
/// paired-device records.
pub struct Storage {
    db: Database,
    config: AppConfig,
    identity: DeviceIdentity,
}

impl Storage {
    /// Open (or create) the persistent store in `config_dir`.
    ///
    /// The redb database file is stored as `storage.redb` inside `config_dir`.
    /// If the database is fresh the schema is bootstrapped automatically.
    pub fn open(config_dir: &Path) -> Result<Self, anyhow::Error> {
        let config = AppConfig::load();
        let identity = DeviceIdentity::load_or_create(config_dir)?;

        std::fs::create_dir_all(config_dir)?;
        let db_path = config_dir.join("storage.redb");
        let db = Database::create(db_path)?;

        let storage = Self {
            db,
            config,
            identity,
        };
        storage.run_migrations()?;
        Ok(storage)
    }

    // -- Public accessors ----------------------------------------------------

    /// The current application configuration (loaded from disk at open time).
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// The local device identity (keypair + node_id).
    pub fn identity(&self) -> &DeviceIdentity {
        &self.identity
    }

    // -- Metadata helpers ----------------------------------------------------

    fn set_metadata_bytes(&self, key: &str, value: &[u8]) -> Result<(), anyhow::Error> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(METADATA)?;
            table.insert(key, value)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    // -- Paired device management --------------------------------------------

    /// Add a new paired device.  If a device with the same `node_id` already
    /// exists it is replaced.
    pub fn add_paired_device(&self, device: &PairedDevice) -> Result<(), anyhow::Error> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(PAIRED_DEVICES)?;
            let key: [u8; 16] = device.node_id.into_bytes();
            let value = bincode::serde::encode_to_vec(device, bincode::config::standard())?;
            table.insert(key.as_slice(), value.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// List all paired devices.
    ///
    /// Returns an empty vector if no devices have been paired yet.
    pub fn paired_devices(&self) -> Result<Vec<PairedDevice>, anyhow::Error> {
        let read_txn = self.db.begin_read()?;
        let table = match read_txn.open_table(PAIRED_DEVICES) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let mut devices = Vec::new();
        for result in table.iter()? {
            let (_key, value) = result?;
            let device = decode_paired_device(value.value())?;
            devices.push(device);
        }
        Ok(devices)
    }

    /// Remove a paired device by its node identifier.
    pub fn remove_paired_device(&self, node_id: &Uuid) -> Result<(), anyhow::Error> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(PAIRED_DEVICES)?;
            let key: [u8; 16] = node_id.into_bytes();
            let _ = table.remove(key.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Check whether a device with the given `node_id` is already paired.
    pub fn has_paired_device(&self, node_id: &Uuid) -> Result<bool, anyhow::Error> {
        let read_txn = self.db.begin_read()?;
        let table = match read_txn.open_table(PAIRED_DEVICES) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let key: [u8; 16] = node_id.into_bytes();
        Ok(table.get(key.as_slice())?.is_some())
    }

    /// Update the `last_seen` timestamp for an existing paired device.
    pub fn update_last_seen(&self, node_id: &Uuid, timestamp_ms: u64) -> Result<(), anyhow::Error> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(PAIRED_DEVICES)?;
            let key: [u8; 16] = node_id.into_bytes();
            // Copy existing bytes into an owned Vec so the AccessGuard (which
            // borrows `table`) is dropped before we call `table.insert()`.
            let existing = {
                let guard = table.get(key.as_slice())?;
                guard.map(|g| g.value().to_vec())
            };
            if let Some(device_bytes) = existing {
                let mut device = decode_paired_device(&device_bytes)?;
                device.last_seen = timestamp_ms;
                let value = bincode::serde::encode_to_vec(&device, bincode::config::standard())?;
                table.insert(key.as_slice(), value.as_slice())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Set the trusted flag on a paired device.
    pub fn set_trusted(&self, node_id: &Uuid, trusted: bool) -> Result<(), anyhow::Error> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(PAIRED_DEVICES)?;
            let key: [u8; 16] = node_id.into_bytes();
            // Copy existing bytes into an owned Vec so the AccessGuard (which
            // borrows `table`) is dropped before we call `table.insert()`.
            let existing = {
                let guard = table.get(key.as_slice())?;
                guard.map(|g| g.value().to_vec())
            };
            if let Some(device_bytes) = existing {
                let mut device = decode_paired_device(&device_bytes)?;
                device.is_trusted = trusted;
                device.trust_state = if trusted {
                    DeviceTrust::Trusted
                } else {
                    DeviceTrust::AskEachTime
                };
                let value = bincode::serde::encode_to_vec(&device, bincode::config::standard())?;
                table.insert(key.as_slice(), value.as_slice())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Set the explicit trust policy on a paired device.
    pub fn set_trust_state(
        &self,
        node_id: &Uuid,
        trust_state: DeviceTrust,
    ) -> Result<(), anyhow::Error> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(PAIRED_DEVICES)?;
            let key: [u8; 16] = node_id.into_bytes();
            let existing = {
                let guard = table.get(key.as_slice())?;
                guard.map(|g| g.value().to_vec())
            };
            if let Some(device_bytes) = existing {
                let mut device = decode_paired_device(&device_bytes)?;
                device.trust_state = trust_state;
                device.is_trusted = matches!(trust_state, DeviceTrust::Trusted);
                let value = bincode::serde::encode_to_vec(&device, bincode::config::standard())?;
                table.insert(key.as_slice(), value.as_slice())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    // -- Schema migration ----------------------------------------------------

    fn run_migrations(&self) -> Result<(), anyhow::Error> {
        // Ensure the metadata table exists.  If the database is brand-new this
        // is the first write and seeds the schema version.
        {
            let write_txn = self.db.begin_write()?;
            {
                let mut metadata = write_txn.open_table(METADATA)?;
                if metadata.is_empty()? {
                    metadata.insert("schema_version", &SCHEMA_VERSION.to_le_bytes().as_slice())?;
                    // Also ensure the paired_devices table exists.
                    drop(metadata);
                    write_txn.open_table(PAIRED_DEVICES)?;
                }
            }
            write_txn.commit()?;
        }

        // Read current schema version.
        let current: u32 = {
            let read_txn = self.db.begin_read()?;
            let table = read_txn.open_table(METADATA)?;
            match table.get("schema_version")? {
                Some(guard) => {
                    let bytes = guard.value();
                    if bytes.len() >= 4 {
                        u32::from_le_bytes(bytes[..4].try_into()?)
                    } else {
                        0
                    }
                }
                None => 0,
            }
        };

        if current < SCHEMA_VERSION {
            self.migrate(current)?;
            // Update schema version.
            self.set_metadata_bytes("schema_version", &SCHEMA_VERSION.to_le_bytes())?;
        }

        log::info!("Storage schema version: {current}");
        Ok(())
    }

    /// Apply migrations from `from_version` to [`SCHEMA_VERSION`].
    fn migrate(&self, from_version: u32) -> Result<(), anyhow::Error> {
        if from_version < 1 {
            log::info!("Migrating storage schema v0 -> v1");

            let write_txn = self.db.begin_write()?;
            {
                // Create the paired_devices table if it does not already exist.
                write_txn.open_table(PAIRED_DEVICES)?;
            }
            write_txn.commit()?;
        }

        if from_version < 2 {
            log::info!("Migrating storage schema v1 -> v2");
            self.migrate_paired_devices_to_trust_state()?;
        }

        Ok(())
    }

    fn migrate_paired_devices_to_trust_state(&self) -> Result<(), anyhow::Error> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(PAIRED_DEVICES)?;
            let updates: Vec<(Vec<u8>, Vec<u8>)> = {
                let mut updates = Vec::new();
                for result in table.iter()? {
                    let (key, value) = result?;
                    let bytes = value.value();
                    let already_v2 = bincode::serde::decode_from_slice::<PairedDevice, _>(
                        bytes,
                        bincode::config::standard(),
                    )
                    .map(|(_, consumed)| consumed == bytes.len())
                    .unwrap_or(false);
                    if already_v2 {
                        continue;
                    }

                    let (old, _): (PairedDeviceV1, _) =
                        bincode::serde::decode_from_slice(bytes, bincode::config::standard())?;
                    let upgraded: PairedDevice = old.into();
                    let encoded =
                        bincode::serde::encode_to_vec(&upgraded, bincode::config::standard())?;
                    updates.push((key.value().to_vec(), encoded));
                }
                updates
            };

            for (key, value) in updates {
                table.insert(key.as_slice(), value.as_slice())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }
}
