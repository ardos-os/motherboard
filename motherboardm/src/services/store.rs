use alloc::{format, vec::Vec};
use core::ops::Deref;

use hashbrown::HashMap;
use motherboardm_protocol::{AnonymousStoreId, ReplyToken, SubscriptionId, TransportError};
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
    anonymous_stores: HashMap<AnonymousStoreId, AnonymousStore>,
    next_anonymous_store_id: u64,
    next_timestamp: isize,
}

impl StoreMap {
    pub fn new() -> Self {
        Self {
            stores: HashMap::new(),
            anonymous_stores: HashMap::new(),
            next_anonymous_store_id: 1,
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

    pub fn create_anonymous(
        &mut self,
        service: SharedStr,
        owner_file_id: FileId,
        initial_value: SharedData,
    ) -> AnonymousStoreSnapshot {
        let id = self.allocate_anonymous_store_id();
        let last_updated_timestamp = self.allocate_timestamp();
        let store = AnonymousStore {
            id,
            service,
            owner_file_id,
            current_value: initial_value,
            last_updated_timestamp,
            pending_count: 0,
            accepted_subscription_count: 0,
            has_ever_had_subscription: false,
        };
        let snapshot = store.snapshot();
        self.anonymous_stores.insert(id, store);
        snapshot
    }

    pub fn update_anonymous(
        &mut self,
        id: AnonymousStoreId,
        value: SharedData,
    ) -> Option<AnonymousStoreSnapshot> {
        let last_updated_timestamp = self.allocate_timestamp();
        let store = self.anonymous_stores.get_mut(&id)?;
        store.current_value = value;
        store.last_updated_timestamp = last_updated_timestamp;
        Some(store.snapshot())
    }

    pub fn get(&self, path: &StorePath) -> Option<&Store> {
        self.stores.get(path)
    }

    pub fn get_anonymous(&self, id: AnonymousStoreId) -> Option<&AnonymousStore> {
        self.anonymous_stores.get(&id)
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

    pub fn remove_anonymous_service(&mut self, service: &str) -> Vec<AnonymousStoreId> {
        let mut removed = Vec::new();
        self.anonymous_stores.retain(|id, store| {
            let keep = store.service.as_str() != service;
            if !keep {
                removed.push(*id);
            }
            keep
        });
        removed
    }

    pub fn increment_anonymous_pending(&mut self, id: AnonymousStoreId) -> Option<()> {
        let store = self.anonymous_stores.get_mut(&id)?;
        store.pending_count = store.pending_count.saturating_add(1);
        Some(())
    }

    pub fn decrement_anonymous_pending(&mut self, id: AnonymousStoreId) {
        if let Some(store) = self.anonymous_stores.get_mut(&id) {
            store.pending_count = store.pending_count.saturating_sub(1);
        }
        self.cleanup_anonymous_if_unused(id);
    }

    pub fn increment_anonymous_accepted(&mut self, id: AnonymousStoreId) -> Option<()> {
        let store = self.anonymous_stores.get_mut(&id)?;
        store.accepted_subscription_count = store.accepted_subscription_count.saturating_add(1);
        store.has_ever_had_subscription = true;
        Some(())
    }

    pub fn accept_anonymous_pending(&mut self, id: AnonymousStoreId) -> Option<()> {
        let store = self.anonymous_stores.get_mut(&id)?;
        store.pending_count = store.pending_count.saturating_sub(1);
        store.accepted_subscription_count = store.accepted_subscription_count.saturating_add(1);
        store.has_ever_had_subscription = true;
        Some(())
    }

    pub fn decrement_anonymous_accepted(&mut self, id: AnonymousStoreId) {
        if let Some(store) = self.anonymous_stores.get_mut(&id) {
            store.accepted_subscription_count = store.accepted_subscription_count.saturating_sub(1);
        }
        self.cleanup_anonymous_if_unused(id);
    }

    fn cleanup_anonymous_if_unused(&mut self, id: AnonymousStoreId) {
        let should_remove = self.anonymous_stores.get(&id).is_some_and(|store| {
            store.has_ever_had_subscription
                && store.pending_count == 0
                && store.accepted_subscription_count == 0
        });
        if should_remove {
            self.anonymous_stores.remove(&id);
        }
    }

    fn allocate_anonymous_store_id(&mut self) -> AnonymousStoreId {
        loop {
            let id = AnonymousStoreId(self.next_anonymous_store_id);
            self.next_anonymous_store_id = self.next_anonymous_store_id.wrapping_add(1);
            if self.next_anonymous_store_id == 0 {
                self.next_anonymous_store_id = 1;
            }

            if !self.anonymous_stores.contains_key(&id) {
                return id;
            }
        }
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

#[derive(Debug)]
pub struct AnonymousStore {
    id: AnonymousStoreId,
    service: SharedStr,
    owner_file_id: FileId,
    current_value: SharedData,
    last_updated_timestamp: isize,
    pending_count: usize,
    accepted_subscription_count: usize,
    has_ever_had_subscription: bool,
}

impl AnonymousStore {
    pub fn id(&self) -> AnonymousStoreId {
        self.id
    }

    pub fn service(&self) -> &SharedStr {
        &self.service
    }

    pub fn owner_file_id(&self) -> FileId {
        self.owner_file_id
    }

    pub fn snapshot(&self) -> AnonymousStoreSnapshot {
        AnonymousStoreSnapshot {
            id: self.id,
            current_value: self.current_value.clone(),
            last_updated_timestamp: self.last_updated_timestamp,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AnonymousStoreSnapshot {
    pub id: AnonymousStoreId,
    pub current_value: SharedData,
    pub last_updated_timestamp: isize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StoreSubscriptionTarget {
    Named(StorePath),
    Anonymous(AnonymousStoreId),
}

#[derive(Clone, Debug)]
struct SubscriptionData {
    target: StoreSubscriptionTarget,
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
        target: StoreSubscriptionTarget,
    ) -> Result<(), TransportError> {
        let key = (file, id);
        if self.map.contains_key(&key) {
            return Err(TransportError::SubscriptionIdConflict);
        }

        self.map.insert(key, SubscriptionData { target, file });
        Ok(())
    }

    pub fn check_ownership(&self, sub: SubscriptionId, file_id: FileId) -> Option<bool> {
        Some(self.map.get(&(file_id, sub))?.file == file_id)
    }

    pub fn delete_owned(
        &mut self,
        id: SubscriptionId,
        file_id: FileId,
    ) -> Result<StoreSubscriptionTarget, TransportError> {
        if self.check_ownership(id, file_id) != Some(true) {
            return Err(TransportError::NoSuchSubscription);
        }

        self.map
            .remove(&(file_id, id))
            .map(|data| data.target)
            .ok_or(TransportError::NoSuchSubscription)
    }

    pub fn subscribers_for_store(&self, store: &StorePath) -> Vec<(FileId, SubscriptionId)> {
        self.map
            .iter()
            .filter_map(|((_, id), data)| {
                if data.target == StoreSubscriptionTarget::Named(store.clone()) {
                    Some((data.file, *id))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn subscribers_for_anonymous_store(
        &self,
        id: AnonymousStoreId,
    ) -> Vec<(FileId, SubscriptionId)> {
        self.map
            .iter()
            .filter_map(|((_, subscription_id), data)| {
                if data.target == StoreSubscriptionTarget::Anonymous(id) {
                    Some((data.file, *subscription_id))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn cleanup_file(
        &mut self,
        file_id: FileId,
    ) -> Vec<(FileId, SubscriptionId, StoreSubscriptionTarget)> {
        let mut removed = Vec::new();
        self.map
            .retain(|(subscription_file, subscription_id), data| {
                let keep = *subscription_file != file_id && data.file != file_id;
                if !keep {
                    removed.push((data.file, *subscription_id, data.target.clone()));
                }
                keep
            });
        removed
    }

    pub fn cleanup_service(
        &mut self,
        service: &str,
    ) -> Vec<(FileId, SubscriptionId, StoreSubscriptionTarget)> {
        let mut removed = Vec::new();
        self.map.retain(|(_, id), data| {
            let keep = match &data.target {
                StoreSubscriptionTarget::Named(path) => path.service.as_str() != service,
                StoreSubscriptionTarget::Anonymous(_) => true,
            };
            if !keep {
                removed.push((data.file, *id, data.target.clone()));
            }
            keep
        });
        removed
    }

    pub fn cleanup_anonymous_store(
        &mut self,
        anonymous_id: AnonymousStoreId,
    ) -> Vec<(FileId, SubscriptionId, StoreSubscriptionTarget)> {
        let mut removed = Vec::new();
        self.map.retain(|(_, id), data| {
            let keep = data.target != StoreSubscriptionTarget::Anonymous(anonymous_id);
            if !keep {
                removed.push((data.file, *id, data.target.clone()));
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
    pub target: StoreSubscriptionTarget,
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
        target: StoreSubscriptionTarget,
    ) -> ReplyToken {
        let token = self.allocate_unique();
        self.pending.insert(
            token,
            PendingStoreSubscription {
                client_file_id,
                service_file_id,
                subscription_id,
                target,
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

    pub fn remove_for_file(&mut self, file_id: FileId) -> Vec<PendingStoreSubscription> {
        let mut removed = Vec::new();
        self.pending.retain(|_, pending| {
            let keep = pending.client_file_id != file_id && pending.service_file_id != file_id;
            if !keep {
                removed.push(pending.clone());
            }
            keep
        });
        removed
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
