use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone)]
pub(super) struct Cache {
    pub directory: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Entry<T> {
    version: u8,
    key: String,
    value: T,
}

impl Cache {
    fn path(&self, slot: &str) -> PathBuf {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        slot.hash(&mut hash);
        self.directory.join(format!("{:016x}.json", hash.finish()))
    }

    pub fn read<T: DeserializeOwned>(&self, slot: &str, key: &str) -> Option<T> {
        let file = std::fs::File::open(self.path(slot)).ok()?;
        let entry: Entry<T> = serde_json::from_reader(std::io::BufReader::new(file)).ok()?;
        (entry.version == 2 && entry.key == key).then_some(entry.value)
    }

    pub fn write<T: Serialize>(&self, slot: &str, key: &str, value: &T) {
        let save = || -> Result<(), Box<dyn std::error::Error>> {
            let mut directory = std::fs::DirBuilder::new();
            directory.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                directory.mode(0o700);
            }
            directory.create(&self.directory)?;
            let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
            serde_json::to_writer(
                &mut file,
                &Entry {
                    version: 2,
                    key: key.into(),
                    value,
                },
            )?;
            file.flush()?;
            file.persist(self.path(slot))?;
            Ok(())
        };
        let _ = save();
    }
}
