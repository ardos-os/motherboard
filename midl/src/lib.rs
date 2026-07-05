pub mod ast;
pub mod codegen;
pub mod parser;
pub mod tokenizer;

pub use ast::{
    Diagnostic, Document, Enum, EnumVariant, Field, Function, Service, ServiceItem, Store, Type,
    TypeAlias,
};
pub use codegen::rust::{RustMode, generate_rust, generate_rust_with_mode};

pub fn parse_document(source: &str) -> Result<Document, Diagnostic> {
    let tokens = tokenizer::tokenize(source)?;
    parser::parse_tokens(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
service Init {
    store totalComponents: string[],
    store startingComponents: string[];
    store startedComponents: string[];
    store logs: string[];
}

service Auth {
    type AuthSessionToken = string;
    type PID = u64;

    enum AuthSessionStatus {
        Pending,
        LoggedIn,
        Canceled
    }

    enum FarProcessState {
        Running,
        ExittedNormally,
        Crashed { logs: string }
    }

    /// This starts a new login session.
    fn start_user_auth(user: string) -> AuthSessionToken?(error { UserNotFound, Conflict });
    fn login_with_password(sessionId: AuthSessionToken, password: string) -> bool?(error { InvalidSessionId });
    fn run_command_as(sessionId: AuthSessionToken) -> PID?(error { InvalidSessionId, CommandSpawnFailed });
    fn cancel_login(sessionId: AuthSessionToken) -> void?(error { NotFound });
    fn auth_session_status(sessionId: AuthSessionToken) -> AnonymousStore<AuthSessionStatus>?(error { InvalidSessionId, PermissionDenied });
    fn watch_far_process(sessionId: AuthSessionToken, pid: PID) -> AnonymousStore<FarProcessState>?(error { InvalidSessionId, PermissionDenied, PidNotFound });
}
"#;

    #[test]
    fn parses_auth_init_example() {
        let document = parse_document(EXAMPLE).unwrap();
        assert_eq!(document.services.len(), 2);
        assert_eq!(document.services[0].name, "Init");
        assert_eq!(document.services[1].name, "Auth");
        assert!(matches!(
            &document.services[1].items[2],
            ServiceItem::Enum(Enum { name, .. }) if name == "AuthSessionStatus"
        ));
    }

    #[test]
    fn generates_rust_for_anonymous_store_return() {
        let document = parse_document(EXAMPLE).unwrap();
        let rust = generate_rust(&document);
        assert!(rust.contains("pub mod auth"));
        assert!(rust.contains("pub struct AuthService;"));
        assert!(rust.contains("pub struct TotalComponentsStore;"));
        assert!(rust.contains("pub struct TotalComponentsStoreSubscription"));
        assert!(rust.contains("pub enum StoreEvent<T>"));
        assert!(rust.contains("pub struct AnonymousStore<T>"));
        assert!(rust.contains("pub async fn auth_session_status"));
        assert!(rust.contains("pub async fn next_message"));
        assert!(rust.contains("state.messages.lock().await.push_back(message);"));
        assert!(rust.contains("pub fn spawn_request"));
        assert!(rust.contains("pub trait AuthServiceHandler"));
        assert!(rust.contains("pub struct ServerRuntime"));
        assert!(rust.contains("pub async fn install_auth"));
        assert!(rust.contains("tokio::spawn"));
        assert!(rust.contains("pub fn create_auth_session_status_store<A>"));
        assert!(rust.contains("authorize_anonymous_subscription"));
        assert!(rust.contains("BindingResult<Result<AuthSessionToken, StartUserAuthError>>"));
        assert!(rust.contains("value: &Result<AuthSessionToken, StartUserAuthError>"));
        assert!(!rust.contains("reject_start_user_auth"));
        assert!(rust.contains("AnonymousStore<AuthSessionStatus>"));
        assert!(rust.contains("pub enum AuthSessionStatus"));
    }
}
