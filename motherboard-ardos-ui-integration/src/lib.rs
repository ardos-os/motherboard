use std::{
    collections::HashMap,
    fmt,
    ops::Deref,
    os::fd::AsRawFd,
    rc::Rc,
    sync::{
        Arc, Mutex, Weak,
        mpsc::{self, Sender},
    },
    thread,
};

pub use ardos_ui;
use ardos_ui::{ExternalStoreSubscription, use_callback, use_sync_external_store};
pub use motherboard_client;
use motherboard_client::{
    ClientApi, ClientError, ClientStoresApi, InboxMessage, MotherboardClient, SubscriptionId,
};

pub type SharedData = Arc<[u8]>;

#[derive(Clone)]
pub struct MotherboardUi {
    inner: Arc<Inner>,
}

struct Inner {
    motherboard: Arc<MotherboardClient>,
    subscriptions: Mutex<HashMap<SubscriptionId, StoreSubscription>>,
}

struct StoreSubscription {
    service: String,
    store: String,
    tx: Sender<SharedData>,
}

#[derive(Debug)]
pub enum MotherboardUiError {
    Open(std::io::Error),
    Client(ClientError),
}

impl fmt::Display for MotherboardUiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => write!(f, "failed to open motherboard device: {error}"),
            Self::Client(error) => write!(f, "motherboard client error: {error}"),
        }
    }
}

impl std::error::Error for MotherboardUiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Open(error) => Some(error),
            Self::Client(error) => Some(error),
        }
    }
}

impl From<ClientError> for MotherboardUiError {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}

impl MotherboardUi {
    pub fn open() -> Result<Self, MotherboardUiError> {
        let motherboard = MotherboardClient::open().map_err(MotherboardUiError::Open)?;
        Ok(Self::from_client(motherboard))
    }

    pub fn from_client(motherboard: MotherboardClient) -> Self {
        let inner = Arc::new(Inner {
            motherboard: Arc::new(motherboard),
            subscriptions: Mutex::new(HashMap::new()),
        });

        spawn_dispatcher(Arc::downgrade(&inner));

        Self { inner }
    }

    pub fn subscribe_store(
        &self,
        service: impl Into<String>,
        store: impl Into<String>,
    ) -> Result<ExternalStoreSubscription<SharedData>, MotherboardUiError> {
        self.subscribe_store_with_payload(service, store, Box::<[u8]>::default())
    }

    pub fn subscribe_store_with_payload(
        &self,
        service: impl Into<String>,
        store: impl Into<String>,
        payload: impl Into<Box<[u8]>>,
    ) -> Result<ExternalStoreSubscription<SharedData>, MotherboardUiError> {
        let service = service.into();
        let store = store.into();
        let subscription_id = self.inner.motherboard.next_subscription_id();
        let (tx, rx) = mpsc::channel();

        {
            let mut subscriptions = self
                .inner
                .subscriptions
                .lock()
                .expect("motherboard UI subscription mutex poisoned");
            subscriptions.insert(
                subscription_id,
                StoreSubscription {
                    service: service.clone(),
                    store: store.clone(),
                    tx,
                },
            );
        }

        if let Err(error) = self.inner.motherboard.client().stores().subscribe_with_id(
            &service,
            &store,
            subscription_id,
            payload,
        ) {
            self.remove_subscription(subscription_id);
            return Err(error.into());
        }

        let inner = Arc::downgrade(&self.inner);
        Ok(ExternalStoreSubscription::new(rx, move || {
            if let Some(inner) = inner.upgrade() {
                let _ = inner
                    .motherboard
                    .client()
                    .stores()
                    .unsubscribe(subscription_id);
                remove_subscription(&inner, subscription_id);
            }
        }))
    }

    fn remove_subscription(&self, subscription_id: SubscriptionId) {
        remove_subscription(&self.inner, subscription_id);
    }
}

impl Deref for MotherboardUi {
    type Target = MotherboardClient;

    fn deref(&self) -> &Self::Target {
        &self.inner.motherboard
    }
}

pub fn use_motherboard_store(
    motherboard: MotherboardUi,
    service: impl Into<String>,
    store: impl Into<String>,
) -> Option<SharedData> {
    use_motherboard_store_with_payload(motherboard, service, store, Box::<[u8]>::default())
}

pub fn use_motherboard_store_with_payload(
    motherboard: MotherboardUi,
    service: impl Into<String>,
    store: impl Into<String>,
    payload: impl Into<Box<[u8]>>,
) -> Option<SharedData> {
    let service = service.into();
    let store = store.into();
    let payload = Arc::<[u8]>::from(payload.into());
    let subscription_key = (
        Arc::as_ptr(&motherboard.inner) as usize,
        service.clone(),
        store.clone(),
        hash_bytes(&payload),
    );

    let subscribe = use_callback(
        {
            let motherboard = motherboard.clone();
            let service = service.clone();
            let store = store.clone();
            let payload = Arc::clone(&payload);

            move || {
                motherboard
                    .subscribe_store_with_payload(
                        service.clone(),
                        store.clone(),
                        Box::<[u8]>::from(payload.as_ref()),
                    )
                    .unwrap_or_else(|_| {
                        let (_tx, rx) = mpsc::channel();
                        ExternalStoreSubscription::new(rx, || {})
                    })
            }
        },
        subscription_key,
    );

    use_sync_external_store::<SharedData, _, _>(Rc::clone(&subscribe), || None)
}

fn spawn_dispatcher(inner: Weak<Inner>) {
    thread::spawn(move || {
        while let Some(inner) = inner.upgrade() {
            match inner.motherboard.client().fetch() {
                Ok(message) => dispatch_message(&inner, message),
                Err(ClientError::WouldBlock(latch_fd)) => {
                    if wait_for_latch(&latch_fd).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

fn dispatch_message(inner: &Inner, message: InboxMessage) {
    match message {
        InboxMessage::StoreSubscriptionAccepted {
            service,
            store,
            subscription_id,
            current_value,
            ..
        } => send_store_value(
            inner,
            &service,
            &store,
            subscription_id,
            Arc::from(current_value),
        ),
        InboxMessage::StoreSubscriptionUpdated {
            service,
            store,
            subscription_id,
            payload,
            ..
        } => send_store_value(inner, &service, &store, subscription_id, Arc::from(payload)),
        InboxMessage::StoreSubscriptionRejected {
            subscription_id, ..
        }
        | InboxMessage::StoreSubscriptionClosed {
            subscription_id, ..
        } => {
            remove_subscription(inner, subscription_id);
        }
        InboxMessage::ServiceClosed { service, .. } => {
            remove_service_subscriptions(inner, &service);
        }
        _ => {}
    }
}

fn send_store_value(
    inner: &Inner,
    service: &str,
    store: &str,
    subscription_id: SubscriptionId,
    value: SharedData,
) {
    let sender = {
        let subscriptions = inner
            .subscriptions
            .lock()
            .expect("motherboard UI subscription mutex poisoned");
        subscriptions
            .get(&subscription_id)
            .filter(|subscription| subscription.service == service && subscription.store == store)
            .map(|subscription| subscription.tx.clone())
    };

    if let Some(sender) = sender {
        let _ = sender.send(value);
    }
}

fn remove_subscription(inner: &Inner, subscription_id: SubscriptionId) {
    let mut subscriptions = inner
        .subscriptions
        .lock()
        .expect("motherboard UI subscription mutex poisoned");
    subscriptions.remove(&subscription_id);
}

fn remove_service_subscriptions(inner: &Inner, service: &str) {
    let mut subscriptions = inner
        .subscriptions
        .lock()
        .expect("motherboard UI subscription mutex poisoned");
    subscriptions.retain(|_, subscription| subscription.service != service);
}

fn wait_for_latch(latch_fd: &impl AsRawFd) -> std::io::Result<()> {
    let mut poll_fd = libc::pollfd {
        fd: latch_fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };

    loop {
        let result = unsafe { libc::poll(&mut poll_fd, 1, -1) };
        if result >= 0 {
            return Ok(());
        }

        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}
