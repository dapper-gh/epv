use std::hash::Hash;
use std::ops::Deref;
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{self, SystemTime};

use mailparse::ParsedMail;

use tokio::fs::{self, File, OpenOptions};
use tokio::io;

use dashmap::DashMap;

pub async fn open_parents(opts: &mut OpenOptions, path: impl AsRef<Path>) -> io::Result<File> {
    let mut buf = path.as_ref().to_path_buf();
    buf.pop();

    fs::create_dir_all(buf).await?;

    opts.open(path).await
}

pub fn traverse_mail<'a>(
    mail: &'a ParsedMail<'a>,
    search: &mut impl FnMut(&ParsedMail) -> bool,
) -> Option<&'a ParsedMail<'a>> {
    if search(mail) {
        return Some(mail);
    }

    for subpart in &mail.subparts {
        if let Some(found) = traverse_mail(subpart, search) {
            return Some(found);
        }
    }

    return None;
}

pub fn unix_ms() -> i64 {
    let (dur, multiplier) = match SystemTime::now().duration_since(time::UNIX_EPOCH) {
        Ok(dur) => (dur, 1),
        Err(_e) => (
            time::UNIX_EPOCH
                .duration_since(SystemTime::now())
                .expect("Neither before nor after Unix epoch"),
            -1,
        ),
    };
    (dur.as_millis() as i64) * multiplier
}

#[derive(Debug, Clone)]
pub struct CacheEntry<V> {
    value: V,
    id: usize,
}
impl<V> Deref for CacheEntry<V> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

#[derive(Debug, Clone)]
pub struct Cache<K: Hash + PartialEq + Eq, V, const N: usize> {
    data: Arc<DashMap<K, CacheEntry<V>>>,
    last_id: Arc<AtomicUsize>,
}
impl<K: Hash + PartialEq + Eq, V, const N: usize> Cache<K, V, N> {
    pub fn insert(&self, key: K, value: V) {
        let id = self.last_id.fetch_add(1, Ordering::Relaxed);
        self.data.insert(key, CacheEntry { value, id });
        if self.data.len() >= N {
            self.data.retain(|_k, v| id.wrapping_sub(v.id) >= N);
        }
    }

    pub fn get(&self, key: &K) -> Option<dashmap::mapref::one::Ref<'_, K, CacheEntry<V>>> {
        self.data.get(key)
    }

    pub fn new() -> Self {
        Cache {
            data: Arc::new(DashMap::new()),
            last_id: Arc::new(AtomicUsize::new(0)),
        }
    }
}
