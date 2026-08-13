use std::path::Path;

use redb::{Database, ReadableTableMetadata, TableDefinition};

use crate::app::config::AppConfig;
use crate::app::identity::DeviceIdentity;

const SCHEMA_VERSION: u32 = 3;
const METADATA: TableDefinition<&str, &[u8]> = TableDefinition::new("metadata");

/// Kept only so old databases remain readable during a fail-safe migration.
/// Version 3 no longer reads or interprets these records: Ed25519 session
/// identity plus the persisted `allow_remote_control` switch replaced the old
/// per-device authorization model. Leaving the opaque table in place avoids a
/// destructive migration and cannot grant remote-control permission.
const LEGACY_PAIRED_DEVICES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("paired_devices");

/// Persistent storage for configuration and the local Ed25519 identity.
pub struct Storage {
    db: Database,
    config: AppConfig,
    identity: DeviceIdentity,
}

impl Storage {
    pub fn open(config_dir: &Path) -> Result<Self, anyhow::Error> {
        let config = AppConfig::load();
        let identity = DeviceIdentity::load_or_create(config_dir)?;

        std::fs::create_dir_all(config_dir)?;
        let db = Database::create(config_dir.join("storage.redb"))?;
        let storage = Self {
            db,
            config,
            identity,
        };
        storage.run_migrations()?;
        Ok(storage)
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn identity(&self) -> &DeviceIdentity {
        &self.identity
    }

    fn set_metadata_bytes(&self, key: &str, value: &[u8]) -> Result<(), anyhow::Error> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(METADATA)?;
            table.insert(key, value)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    fn run_migrations(&self) -> Result<(), anyhow::Error> {
        {
            let write_txn = self.db.begin_write()?;
            {
                let mut metadata = write_txn.open_table(METADATA)?;
                if metadata.is_empty()? {
                    metadata.insert("schema_version", &SCHEMA_VERSION.to_le_bytes().as_slice())?;
                    drop(metadata);
                    // Retain the historic table name so opening a database made
                    // by older builds never requires destructive schema work.
                    write_txn.open_table(LEGACY_PAIRED_DEVICES)?;
                }
            }
            write_txn.commit()?;
        }

        let current = {
            let read_txn = self.db.begin_read()?;
            let table = read_txn.open_table(METADATA)?;
            match table.get("schema_version")? {
                Some(guard) if guard.value().len() >= 4 => {
                    u32::from_le_bytes(guard.value()[..4].try_into()?)
                }
                _ => 0,
            }
        };

        if current < SCHEMA_VERSION {
            self.migrate(current)?;
            self.set_metadata_bytes("schema_version", &SCHEMA_VERSION.to_le_bytes())?;
        }
        log::info!("Storage schema version: {}", current.max(SCHEMA_VERSION));
        Ok(())
    }

    fn migrate(&self, from_version: u32) -> Result<(), anyhow::Error> {
        if from_version < 1 {
            let write_txn = self.db.begin_write()?;
            write_txn.open_table(LEGACY_PAIRED_DEVICES)?;
            write_txn.commit()?;
        }
        if from_version < 3 {
            // Intentionally do not decode, rewrite, or delete legacy records.
            // Corrupt historical authorization data therefore cannot prevent
            // the local calculator from starting and cannot become permission.
            log::info!("Migrating storage to v3: legacy per-device authorization is ignored");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_legacy_authorization_rows_do_not_break_local_storage() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("storage.redb");
        {
            let db = Database::create(&db_path).unwrap();
            let write_txn = db.begin_write().unwrap();
            {
                let mut metadata = write_txn.open_table(METADATA).unwrap();
                metadata
                    .insert("schema_version", &2u32.to_le_bytes().as_slice())
                    .unwrap();
                drop(metadata);
                let mut legacy = write_txn.open_table(LEGACY_PAIRED_DEVICES).unwrap();
                legacy
                    .insert(&[7u8; 16][..], &[0xff, 0x00, 0xaa][..])
                    .unwrap();
            }
            write_txn.commit().unwrap();
        }

        let storage = Storage::open(dir.path()).expect("legacy bytes must remain opaque");
        let read_txn = storage.db.begin_read().unwrap();
        let metadata = read_txn.open_table(METADATA).unwrap();
        let version = metadata.get("schema_version").unwrap().unwrap();
        assert_eq!(version.value(), SCHEMA_VERSION.to_le_bytes());
        let legacy = read_txn.open_table(LEGACY_PAIRED_DEVICES).unwrap();
        assert_eq!(
            legacy.len().unwrap(),
            1,
            "migration must remain non-destructive"
        );
    }
}
