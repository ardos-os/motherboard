use alloc::{format, vec::Vec};
use core::ops::Deref;

use hashbrown::HashMap;
use motherboardm_protocol::{ReplyToken, SubscriptionId, TransportError};
use thiserror::Error;

use crate::{SharedData, SharedStr, motherboard_device::FileId};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoreName(SharedStr);

impl StoreName {
    const MAX_STORE_NAME: usize = 255;

    pub fn check(name: impl Into<SharedStr>) -> Result<Self, InvalidStoreName> {
        let name = name.into();
        if name.is_empty() {
            return Err(InvalidStoreName {
                message: "name cannot be empty".into(),
            });
        }
        if name.len() > Self::MAX_STORE_NAME {
            return Err(InvalidStoreName {
                message: format!(
                    "name cannot be longer than {} characters",
                    Self::MAX_STORE_NAME
                )
                .into(),
            });
        }
        if !name
            .chars()
            .all(|c| matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-'))
        {
            return Err(InvalidStoreName {
                message: "invalid characters found: allowed chars: 'a'..'z' | 'A'..'Z' | '0'..'9' | '_' | '-'".into(),
            });
        }

        Ok(Self(name))
    }

    pub fn as_shared(&self) -> &SharedStr {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Deref for StoreName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Error, Debug, Clone)]
#[error("invalid store name: {message}")]
pub struct InvalidStoreName {
    pub message: SharedStr,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StorePath {
    pub service: SharedStr,
    pub store: StoreName,
}

impl StorePath {
    pub fn new(service: impl Into<SharedStr>, store: StoreName) -> Self {
        Self {
            service: service.into(),
            store,
        }
    }
}

#[derive(Debug)]
pub struct StoreMap {
    stores: HashMap<StorePath, Store>,
    next_timestamp: isize,
}

impl StoreMap {
    pub fn new() -> Self {
        Self {
            stores: HashMap::new(),
            next_timestamp: 1,
        }
    }

    pub fn try_register(
        &mut self,
        path: StorePath,
        initial_value: SharedData,
        public: bool,
    ) -> Result<StoreSnapshot, StoreAlreadyExists> {
        if self.stores.contains_key(&path) {
            return Err(StoreAlreadyExists);
        }

        let last_updated_timestamp = self.allocate_timestamp();
        let store = Store {
            path: path.clone(),
            current_value: initial_value,
            public,
            last_updated_timestamp,
        };
        let snapshot = store.snapshot();
        self.stores.insert(path, store);
        Ok(snapshot)
    }

    pub fn update(&mut self, path: &StorePath, value: SharedData) -> Option<StoreSnapshot> {
        let last_updated_timestamp = self.allocate_timestamp();
        let store = self.stores.get_mut(path)?;
        debug_assert_eq!(&store.path, path);
        store.current_value = value;
        store.last_updated_timestamp = last_updated_timestamp;
        Some(store.snapshot())
    }

    pub fn get(&self, path: &StorePath) -> Option<&Store> {
        self.stores.get(path)
    }

    pub fn remove_service(&mut self, service: &str) -> Vec<StorePath> {
        let mut removed = Vec::new();
        self.stores.retain(|path, _| {
            let keep = path.service.as_str() != service;
            if !keep {
                removed.push(path.clone());
            }
            keep
        });
        removed
    }

    fn allocate_timestamp(&mut self) -> isize {
        let timestamp = self.next_timestamp;
        self.next_timestamp = self.next_timestamp.wrapping_add(1);
        if self.next_timestamp <= 0 {
            self.next_timestamp = 1;
        }
        timestamp
    }
}

impl Default for StoreMap {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Error, Debug, Copy, Clone, PartialEq, Eq)]
#[error("store already exists")]
pub struct StoreAlreadyExists;

#[derive(Debug)]
pub struct Store {
    path: StorePath,
    current_value: SharedData,
    public: bool,
    last_updated_timestamp: isize,
}

impl Store {
    pub fn path(&self) -> &StorePath {
        &self.path
    }

    pub fn is_public(&self) -> bool {
        self.public
    }

    pub fn snapshot(&self) -> StoreSnapshot {
        StoreSnapshot {
            path: self.path.clone(),
            current_value: self.current_value.clone(),
            last_updated_timestamp: self.last_updated_timestamp,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StoreSnapshot {
    pub path: StorePath,
    pub current_value: SharedData,
    pub last_updated_timestamp: isize,
}

#[derive(Clone, Debug)]
struct SubscriptionData {
    store: StorePath,
    file: FileId,
}

pub struct Subscriptions {
    map: HashMap<(FileId, SubscriptionId), SubscriptionData>,
}

impl Subscriptions {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn insert(
        &mut self,
        id: SubscriptionId,
        file: FileId,
        store: StorePath,
    ) -> Result<(), TransportError> {
        let key = (file, id);
        if self.map.contains_key(&key) {
            return Err(TransportError::SubscriptionIdConflict);
        }

        self.map.insert(key, SubscriptionData { store, file });
        Ok(())
    }

    pub fn check_ownership(&self, sub: SubscriptionId, file_id: FileId) -> Option<bool> {
        Some(self.map.get(&(file_id, sub))?.file == file_id)
    }

    pub fn delete_owned(
        &mut self,
        id: SubscriptionId,
        file_id: FileId,
    ) -> Result<StorePath, TransportError> {
        if self.check_ownership(id, file_id) != Some(true) {
            return Err(TransportError::NoSuchSubscription);
        }

        self.map
            .remove(&(file_id, id))
            .map(|data| data.store)
            .ok_or(TransportError::NoSuchSubscription)
    }

    pub fn subscribers_for_store(&self, store: &StorePath) -> Vec<(FileId, SubscriptionId)> {
        self.map
            .iter()
            .filter_map(|((_, id), data)| {
                if data.store == *store {
                    Some((data.file, *id))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn cleanup_file(&mut self, file_id: FileId) {
        self.map.retain(|(subscription_file, _), data| {
            *subscription_file != file_id && data.file != file_id
        });
    }

    pub fn cleanup_service(&mut self, service: &str) -> Vec<(FileId, SubscriptionId, StorePath)> {
        let mut removed = Vec::new();
        self.map.retain(|(_, id), data| {
            let keep = data.store.service.as_str() != service;
            if !keep {
                removed.push((data.file, *id, data.store.clone()));
            }
            keep
        });
        removed
    }
}

impl Default for Subscriptions {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub struct PendingStoreSubscription {
    pub client_file_id: FileId,
    pub service_file_id: FileId,
    pub subscription_id: SubscriptionId,
    pub store: StorePath,
}

pub struct StoreSubscriptionReplyTokens {
    next: u64,
    pending: HashMap<ReplyToken, PendingStoreSubscription>,
}

impl StoreSubscriptionReplyTokens {
    pub fn new() -> Self {
        Self {
            next: 1,
            pending: HashMap::new(),
        }
    }

    pub fn create(
        &mut self,
        client_file_id: FileId,
        service_file_id: FileId,
        subscription_id: SubscriptionId,
        store: StorePath,
    ) -> ReplyToken {
        let token = self.allocate_unique();
        self.pending.insert(
            token,
            PendingStoreSubscription {
                client_file_id,
                service_file_id,
                subscription_id,
                store,
            },
        );
        token
    }

    pub fn consume(
        &mut self,
        token: ReplyToken,
        service_file_id: FileId,
    ) -> Result<PendingStoreSubscription, TransportError> {
        let Some(pending) = self.pending.get(&token).cloned() else {
            return Err(TransportError::InvalidReplyToken);
        };

        if pending.service_file_id != service_file_id {
            return Err(TransportError::InvalidReplyToken);
        }

        self.pending.remove(&token);
        Ok(pending)
    }

    pub fn remove_for_file(&mut self, file_id: FileId) {
        self.pending.retain(|_, pending| {
            pending.client_file_id != file_id && pending.service_file_id != file_id
        });
    }

    fn allocate_unique(&mut self) -> ReplyToken {
        loop {
            let token = ReplyToken(self.next);
            self.next = self.next.wrapping_add(1);
            if self.next == 0 {
                self.next = 1;
            }

            if !self.pending.contains_key(&token) {
                return token;
            }
        }
    }
}

impl Default for StoreSubscriptionReplyTokens {
    fn default() -> Self {
        Self::new()
    }
}
